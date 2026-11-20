//! Deterministic third-party license inventory generation.
//!
//! The release script runs `cargo metadata --format-version 1 --locked
//! --offline` for the root workspace and the standalone Zed extension
//! workspace, then hands the JSON documents to [`generate`]. The generator
//! walks the normal dependency edges of the packaged binaries, records every
//! crate's name, version and license expression, and collects byte-identical
//! `LICENSE*`/`NOTICE*`/`COPYING*`/`UNLICENSE*` texts from the local Cargo
//! package directories.
//!
//! Some published crates omit their license files. When a crate has no local
//! text, the generator falls back to canonical SPDX texts vendored under
//! `assets/spdx/` and selected by the recorded license expression. The
//! inventory records the provenance of every text as `local` or `spdx`, so a
//! fallback is never silent. Vendored texts carry no crate-specific copyright
//! line; the rendered inventory states that notice obligations remain with the
//! upstream distribution.
//!
//! Output is deterministic: crates and texts are ordered, duplicate texts are
//! emitted once, and the only timestamp is the caller-supplied
//! `SOURCE_DATE_EPOCH`. A crate with neither a license expression nor a local
//! license text fails the build unless the caller explicitly opts into a
//! local-only inventory with `allow_unlicensed`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{DeliveryError, Result};
use crate::manifest::rfc3339_from_unix;

/// Format identifier written into the generated inventory.
pub const GENERATOR_ID: &str = "wxml-lsp-license-generator";
/// Format version written into the generated inventory.
pub const GENERATOR_VERSION: u64 = 2;
/// Maximum accepted size of one license text file.
const MAX_LICENSE_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum accepted total size of deduplicated license texts.
const MAX_LICENSE_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// Case-insensitive file name prefixes treated as license texts.
const LICENSE_PREFIXES: &[&str] = &["LICENSE", "NOTICE", "COPYING", "UNLICENSE"];

/// One canonical SPDX text vendored with the generator.
///
/// The files under `assets/spdx/` are byte-for-byte copies of
/// `https://github.com/spdx/license-list-data/tree/main/text`. They are
/// embedded so generation never needs the network, and they are only used for
/// crates whose package directory contains no license text.
struct VendoredText {
    /// SPDX identifier as recorded in a Cargo license expression.
    id: &'static str,
    /// File name below `assets/spdx/`, also used as the inventory label.
    file_name: &'static str,
    /// Canonical SPDX license or exception text.
    bytes: &'static [u8],
}

/// Canonical texts available to the expression-based fallback.
const VENDORED_TEXTS: &[VendoredText] = &[
    VendoredText {
        id: "Apache-2.0",
        file_name: "Apache-2.0.txt",
        bytes: include_bytes!("../assets/spdx/Apache-2.0.txt"),
    },
    VendoredText {
        id: "LLVM-exception",
        file_name: "LLVM-exception.txt",
        bytes: include_bytes!("../assets/spdx/LLVM-exception.txt"),
    },
    VendoredText {
        id: "MIT",
        file_name: "MIT.txt",
        bytes: include_bytes!("../assets/spdx/MIT.txt"),
    },
    VendoredText {
        id: "Unicode-3.0",
        file_name: "Unicode-3.0.txt",
        bytes: include_bytes!("../assets/spdx/Unicode-3.0.txt"),
    },
    VendoredText {
        id: "Zlib",
        file_name: "Zlib.txt",
        bytes: include_bytes!("../assets/spdx/Zlib.txt"),
    },
];

/// One `cargo metadata` document and the packages to traverse from.
#[derive(Clone, Debug)]
pub struct MetadataRoot {
    /// Path to a `cargo metadata --format-version 1` JSON document.
    pub metadata: PathBuf,
    /// Human-readable workspace label used in the inventory.
    pub label: String,
    /// Cargo target triple the metadata was filtered for.
    pub target: String,
    /// Root package names whose normal dependencies form this graph.
    pub packages: Vec<String>,
}

/// Inputs for one deterministic license inventory.
#[derive(Clone, Debug)]
pub struct LicenseRequest {
    /// Dependency graphs to merge, in report order.
    pub roots: Vec<MetadataRoot>,
    /// Source commit recorded in the inventory header.
    pub source_commit: String,
    /// Reproducible build timestamp in Unix seconds.
    pub source_date_epoch: u64,
    /// Accept crates without a license expression and without local text.
    ///
    /// This is a local-only escape hatch; release builds must leave it off.
    pub allow_unlicensed: bool,
}

/// Counts describing one generated inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LicenseSummary {
    /// Unique third-party `(name, version)` packages.
    pub third_party_crates: usize,
    /// Third-party crates with at least one license text.
    pub crates_with_text: usize,
    /// Third-party crates whose texts all came from the local package directory.
    pub crates_with_local_text: usize,
    /// Third-party crates whose texts came from vendored SPDX canonical texts.
    pub crates_with_spdx_text: usize,
    /// Third-party crates whose license text is not in the local cache.
    pub crates_without_text: usize,
    /// Third-party crates with no expression and no local text.
    pub unlicensed_crates: usize,
    /// Distinct byte-identical license texts emitted once.
    pub distinct_texts: usize,
    /// Packages built from this repository.
    pub first_party_crates: usize,
    /// Whether a first-party package has no declared license or license file.
    pub first_party_blocked: bool,
}

/// Generates the deterministic `THIRD_PARTY_LICENSES.md` bytes.
///
/// # Errors
///
/// Returns an error for an unreadable or malformed `cargo metadata` document,
/// an empty or invalid dependency graph, a missing declared license file, an
/// oversized license text, or a crate with neither a license expression nor a
/// local license text when `allow_unlicensed` is false.
#[allow(clippy::too_many_lines)] // linear inventory assembly; splitting obscures the checks
pub fn generate(request: &LicenseRequest) -> Result<(Vec<u8>, LicenseSummary)> {
    validate_request(request)?;
    let mut crates: BTreeMap<PackageKey, CrateRecord> = BTreeMap::new();
    let mut graphs: Vec<GraphSummary> = Vec::with_capacity(request.roots.len());
    for root in &request.roots {
        let document = read_metadata(&root.metadata)?;
        let reachable = reachable_packages(&document, &root.packages, &root.label)?;
        let packages: BTreeMap<&str, &MetadataPackage> = document
            .packages
            .iter()
            .map(|package| (package.id.as_str(), package))
            .collect();
        let mut third_party = 0_usize;
        for id in &reachable {
            let package = packages.get(id.as_str()).ok_or_else(|| {
                DeliveryError::InvalidPackage(format!(
                    "cargo metadata resolve references unknown package `{id}`"
                ))
            })?;
            let key = PackageKey {
                name: package.name.clone(),
                version: package.version.clone(),
            };
            let record = crates.entry(key).or_insert_with(|| CrateRecord {
                name: package.name.clone(),
                version: package.version.clone(),
                first_party: package.source.is_none(),
                expressions: BTreeSet::new(),
                declared_files: BTreeSet::new(),
                manifest_dirs: BTreeSet::new(),
                workspaces: BTreeSet::new(),
            });
            if record.first_party != package.source.is_none() {
                return Err(DeliveryError::InvalidPackage(format!(
                    "crate `{} {}` is first-party in one dependency graph and third-party in another",
                    package.name, package.version
                )));
            }
            if let Some(expression) = package.license.as_deref() {
                if !expression.trim().is_empty() {
                    record.expressions.insert(expression.trim().to_owned());
                }
            }
            if let Some(file) = package.license_file.as_deref() {
                record.declared_files.insert(file.to_path_buf());
            }
            record.manifest_dirs.insert(
                package
                    .manifest_path
                    .parent()
                    .map_or_else(|| package.manifest_path.clone(), Path::to_path_buf),
            );
            record.workspaces.insert(root.label.clone());
            if !record.first_party {
                third_party += 1;
            }
        }
        graphs.push(GraphSummary {
            label: root.label.clone(),
            target: root.target.clone(),
            packages: root.packages.clone(),
            third_party_crates: third_party,
        });
    }

    let mut texts: BTreeMap<String, TextRecord> = BTreeMap::new();
    let mut missing: Vec<MissingText> = Vec::new();
    let mut unlicensed: Vec<String> = Vec::new();
    let mut crates_with_text = 0_usize;
    let mut crates_with_local_text = 0_usize;
    let mut crates_with_spdx_text = 0_usize;
    let mut first_party_blocked = false;
    let mut text_bytes = 0_u64;
    let mut rows: Vec<InventoryRow> = Vec::new();
    let mut first_party_rows: Vec<FirstPartyRow> = Vec::new();

    for record in crates.values() {
        let mut crate_texts: BTreeMap<String, LicenseText> = BTreeMap::new();
        for directory in &record.manifest_dirs {
            for text in scan_license_texts(directory)? {
                crate_texts.entry(text.sha256.clone()).or_insert(text);
            }
        }
        for declared in &record.declared_files {
            let text = read_declared_license_file(record, declared)?;
            crate_texts.entry(text.sha256.clone()).or_insert(text);
        }

        let expressions: Vec<String> = record.expressions.iter().cloned().collect();
        let label = format!("{} {}", record.name, record.version);
        if record.first_party {
            if expressions.is_empty() && crate_texts.is_empty() {
                first_party_blocked = true;
            }
            first_party_rows.push(FirstPartyRow {
                name: record.name.clone(),
                version: record.version.clone(),
                expressions,
                has_text: !crate_texts.is_empty(),
            });
            continue;
        }
        let unlicensed_crate = crate_texts.is_empty() && expressions.is_empty();
        if unlicensed_crate {
            unlicensed.push(label);
            if !request.allow_unlicensed {
                continue;
            }
        }
        // Local package files win; only a crate with no local text at all
        // falls back to the canonical SPDX texts named by its expression.
        let mut text_source = if crate_texts.is_empty() {
            None
        } else {
            Some(TextSource::Local)
        };
        if text_source.is_none() {
            for expression in &expressions {
                for text in vendored_license_texts(expression) {
                    crate_texts.entry(text.sha256.clone()).or_insert(text);
                }
            }
            if !crate_texts.is_empty() {
                text_source = Some(TextSource::Spdx);
            }
        }
        if crate_texts.is_empty() {
            if !unlicensed_crate {
                missing.push(MissingText {
                    name: record.name.clone(),
                    version: record.version.clone(),
                    expressions: expressions.clone(),
                });
            }
        } else {
            crates_with_text += 1;
            match text_source {
                Some(TextSource::Local) => crates_with_local_text += 1,
                Some(TextSource::Spdx) => crates_with_spdx_text += 1,
                None => {}
            }
        }
        let text_count = crate_texts.len();
        for text in crate_texts.into_values() {
            let applies = format!("`{}` `{}`", record.name, record.version);
            if let Some(entry) = texts.get_mut(&text.sha256) {
                entry.applies_to.insert(applies);
                continue;
            }
            text_bytes = text_bytes
                .checked_add(u64::try_from(text.bytes.len()).map_err(|_| {
                    DeliveryError::SizeLimit("license text size overflows".to_owned())
                })?)
                .ok_or_else(|| {
                    DeliveryError::SizeLimit("license text size overflows".to_owned())
                })?;
            if text_bytes > MAX_LICENSE_TOTAL_BYTES {
                return Err(DeliveryError::SizeLimit(format!(
                    "deduplicated license texts exceed {MAX_LICENSE_TOTAL_BYTES} bytes"
                )));
            }
            let mut applies_to = BTreeSet::new();
            applies_to.insert(applies);
            let text_label = match text.source {
                TextSource::Local => {
                    format!("{}-{}/{}", record.name, record.version, text.file_name)
                }
                TextSource::Spdx => format!("spdx/{}", text.file_name),
            };
            texts.insert(
                text.sha256.clone(),
                TextRecord {
                    label: text_label,
                    bytes: text.bytes,
                    applies_to,
                    source: text.source,
                    spdx_id: text.spdx_id,
                },
            );
        }
        rows.push(InventoryRow {
            name: record.name.clone(),
            version: record.version.clone(),
            expressions,
            workspaces: record.workspaces.iter().cloned().collect(),
            text_count,
            text_source,
        });
    }

    if !unlicensed.is_empty() && !request.allow_unlicensed {
        return Err(DeliveryError::InvalidPackage(format!(
            "no license expression and no local license text for: {}; \
             set ALLOW_UNLICENSED=1 only for a non-publishable local build",
            unlicensed.join(", ")
        )));
    }

    let summary = LicenseSummary {
        third_party_crates: rows.len(),
        crates_with_text,
        crates_with_local_text,
        crates_with_spdx_text,
        crates_without_text: rows.len() - crates_with_text,
        unlicensed_crates: unlicensed.len(),
        distinct_texts: texts.len(),
        first_party_crates: first_party_rows.len(),
        first_party_blocked,
    };
    let bytes = render(
        request,
        &graphs,
        &rows,
        &texts,
        &missing,
        &unlicensed,
        &first_party_rows,
    );
    Ok((bytes, summary))
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct PackageKey {
    name: String,
    version: String,
}

#[derive(Clone, Debug)]
struct CrateRecord {
    name: String,
    version: String,
    first_party: bool,
    expressions: BTreeSet<String>,
    declared_files: BTreeSet<PathBuf>,
    manifest_dirs: BTreeSet<PathBuf>,
    workspaces: BTreeSet<String>,
}

#[derive(Clone, Debug)]
struct GraphSummary {
    label: String,
    target: String,
    packages: Vec<String>,
    third_party_crates: usize,
}

#[derive(Clone, Debug)]
struct InventoryRow {
    name: String,
    version: String,
    expressions: Vec<String>,
    workspaces: Vec<String>,
    text_count: usize,
    text_source: Option<TextSource>,
}

#[derive(Clone, Debug)]
struct FirstPartyRow {
    name: String,
    version: String,
    expressions: Vec<String>,
    has_text: bool,
}

#[derive(Clone, Debug)]
struct MissingText {
    name: String,
    version: String,
    expressions: Vec<String>,
}

/// Where a license text came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextSource {
    /// Read from the crate's package directory in the local Cargo cache.
    Local,
    /// Vendored canonical SPDX text selected by the license expression.
    Spdx,
}

impl TextSource {
    const fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Spdx => "spdx",
        }
    }
}

#[derive(Clone, Debug)]
struct LicenseText {
    file_name: String,
    sha256: String,
    bytes: Vec<u8>,
    source: TextSource,
    /// SPDX identifier of a vendored text; always absent for local texts.
    spdx_id: Option<&'static str>,
}

#[derive(Clone, Debug)]
struct TextRecord {
    label: String,
    bytes: Vec<u8>,
    applies_to: BTreeSet<String>,
    source: TextSource,
    spdx_id: Option<&'static str>,
}

#[derive(Deserialize)]
struct MetadataDocument {
    packages: Vec<MetadataPackage>,
    resolve: Option<MetadataResolve>,
}

#[derive(Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    version: String,
    license: Option<String>,
    license_file: Option<PathBuf>,
    manifest_path: PathBuf,
    source: Option<String>,
}

#[derive(Deserialize)]
struct MetadataResolve {
    nodes: Vec<MetadataNode>,
}

#[derive(Deserialize)]
struct MetadataNode {
    id: String,
    #[serde(default)]
    deps: Vec<MetadataDependency>,
}

#[derive(Deserialize)]
struct MetadataDependency {
    pkg: String,
    #[serde(default)]
    dep_kinds: Vec<MetadataDepKind>,
}

#[derive(Deserialize)]
struct MetadataDepKind {
    kind: Option<String>,
}

fn validate_request(request: &LicenseRequest) -> Result<()> {
    if request.roots.is_empty() {
        return Err(DeliveryError::InvalidArgument(
            "at least one --metadata dependency graph is required".to_owned(),
        ));
    }
    if request.source_commit.len() < 7
        || request.source_commit.len() > 64
        || !request
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DeliveryError::InvalidArgument(
            "source commit must be a 7-64 character hexadecimal commit id".to_owned(),
        ));
    }
    for root in &request.roots {
        if root.packages.is_empty() {
            return Err(DeliveryError::InvalidArgument(format!(
                "dependency graph `{}` lists no root packages",
                root.label
            )));
        }
        if root.label.is_empty()
            || root.label.chars().any(char::is_control)
            || root.target.is_empty()
            || !root
                .target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(DeliveryError::InvalidArgument(
                "workspace labels must be non-empty single-line text and targets must be Cargo triples"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

fn read_metadata(path: &Path) -> Result<MetadataDocument> {
    let bytes = std::fs::read(path).map_err(|error| DeliveryError::io(path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!(
            "cargo metadata document `{}` is malformed: {error}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<metadata>")
        ))
    })
}

fn reachable_packages(
    document: &MetadataDocument,
    roots: &[String],
    label: &str,
) -> Result<BTreeSet<String>> {
    let resolve = document.resolve.as_ref().ok_or_else(|| {
        DeliveryError::InvalidPackage(format!(
            "cargo metadata for `{label}` has no resolved dependency graph"
        ))
    })?;
    let packages: BTreeMap<&str, &MetadataPackage> = document
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    let nodes: BTreeMap<&str, &MetadataNode> = resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut queue: Vec<String> = Vec::new();
    for name in roots {
        let matches: Vec<&MetadataPackage> = document
            .packages
            .iter()
            .filter(|package| &package.name == name)
            .collect();
        match matches.as_slice() {
            [package] => queue.push(package.id.clone()),
            [] => {
                return Err(DeliveryError::InvalidPackage(format!(
                    "cargo metadata for `{label}` has no root package `{name}`"
                )));
            }
            _ => {
                return Err(DeliveryError::InvalidPackage(format!(
                    "cargo metadata for `{label}` has more than one package named `{name}`"
                )));
            }
        }
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let node = nodes.get(id.as_str()).ok_or_else(|| {
            DeliveryError::InvalidPackage(format!(
                "cargo metadata resolve for `{label}` has no node `{id}`"
            ))
        })?;
        for dependency in &node.deps {
            if !dependency.dep_kinds.is_empty()
                && !dependency
                    .dep_kinds
                    .iter()
                    .any(|kind| kind.kind.as_deref().is_none_or(|kind| kind == "normal"))
            {
                continue;
            }
            queue.push(dependency.pkg.clone());
        }
    }
    for id in &seen {
        if !packages.contains_key(id.as_str()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "cargo metadata for `{label}` resolved an unknown package `{id}`"
            )));
        }
    }
    Ok(seen)
}

fn scan_license_texts(directory: &Path) -> Result<Vec<LicenseText>> {
    let canonical = std::fs::canonicalize(directory).map_err(|error| {
        DeliveryError::InvalidPackage(format!(
            "license directory `{}` is unavailable: {error}",
            directory.display()
        ))
    })?;
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    let entries =
        std::fs::read_dir(&canonical).map_err(|error| DeliveryError::io(&canonical, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(&canonical, error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let upper = name.to_ascii_uppercase();
        if !LICENSE_PREFIXES
            .iter()
            .any(|prefix| upper.starts_with(prefix))
        {
            continue;
        }
        let path = entry.path();
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(DeliveryError::io(&path, error)),
        };
        if !metadata.is_file() || metadata.len() == 0 {
            continue;
        }
        if metadata.len() > MAX_LICENSE_FILE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "license text `{name}` exceeds {MAX_LICENSE_FILE_BYTES} bytes"
            )));
        }
        let path = std::fs::canonicalize(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if !path.starts_with(&canonical) {
            return Err(DeliveryError::UnsafeArchive(format!(
                "license text `{name}` resolves outside its package directory"
            )));
        }
        candidates.push((name, path));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    let mut texts: BTreeMap<String, LicenseText> = BTreeMap::new();
    for (file_name, path) in candidates {
        let bytes = std::fs::read(&path).map_err(|error| DeliveryError::io(&path, error))?;
        let sha256 = sha256_hex(&bytes);
        texts.entry(sha256.clone()).or_insert(LicenseText {
            file_name,
            sha256,
            bytes,
            source: TextSource::Local,
            spdx_id: None,
        });
    }
    Ok(texts.into_values().collect())
}

fn read_declared_license_file(record: &CrateRecord, declared: &Path) -> Result<LicenseText> {
    if declared.is_absolute() {
        return Err(DeliveryError::InvalidPackage(format!(
            "crate `{} {}` declares an absolute license file",
            record.name, record.version
        )));
    }
    for directory in &record.manifest_dirs {
        let path = directory.join(declared);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(DeliveryError::io(&path, error)),
        };
        if !metadata.is_file() || metadata.len() == 0 {
            continue;
        }
        if metadata.len() > MAX_LICENSE_FILE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "declared license file `{}` exceeds {MAX_LICENSE_FILE_BYTES} bytes",
                declared.display()
            )));
        }
        let bytes = std::fs::read(&path).map_err(|error| DeliveryError::io(&path, error))?;
        return Ok(LicenseText {
            file_name: declared.to_string_lossy().into_owned(),
            sha256: sha256_hex(&bytes),
            bytes,
            source: TextSource::Local,
            spdx_id: None,
        });
    }
    Err(DeliveryError::Integrity(format!(
        "crate `{} {}` declares license file `{}` but it is missing from the local cache",
        record.name,
        record.version,
        declared.display()
    )))
}

/// Returns the vendored canonical texts named by one Cargo license expression.
///
/// Every recognizable identifier is included instead of choosing between
/// alternatives: `OR` lists, `AND` conjunctions and `X WITH Y` both contribute
/// their texts. Unknown identifiers are skipped; the caller keeps the crate in
/// the explicit "text unavailable" section when nothing matches.
fn vendored_license_texts(expression: &str) -> Vec<LicenseText> {
    let mut texts = Vec::new();
    let mut seen = BTreeSet::new();
    for id in expression_identifiers(expression) {
        let Some(vendored) = vendored_text(&id) else {
            continue;
        };
        let sha256 = sha256_hex(vendored.bytes);
        if !seen.insert(sha256.clone()) {
            continue;
        }
        texts.push(LicenseText {
            file_name: vendored.file_name.to_owned(),
            sha256,
            bytes: vendored.bytes.to_vec(),
            source: TextSource::Spdx,
            spdx_id: Some(vendored.id),
        });
    }
    texts
}

fn expression_identifiers(expression: &str) -> Vec<String> {
    let tokens: Vec<&str> = expression
        .split(|character: char| character.is_whitespace() || matches!(character, '(' | ')'))
        .filter(|token| !token.is_empty())
        .collect();
    let mut identifiers = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if matches!(token, "OR" | "AND" | "WITH") {
            index += 1;
            continue;
        }
        identifiers.push(token.to_owned());
        if tokens
            .get(index + 1)
            .is_some_and(|operator| *operator == "WITH")
        {
            if let Some(exception) = tokens.get(index + 2) {
                identifiers.push((*exception).to_owned());
                index += 3;
                continue;
            }
        }
        index += 1;
    }
    identifiers
}

fn vendored_text(id: &str) -> Option<&'static VendoredText> {
    VENDORED_TEXTS
        .iter()
        .find(|text| text.id.eq_ignore_ascii_case(id))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[allow(clippy::too_many_lines)] // one section at a time keeps the Markdown stable
fn render(
    request: &LicenseRequest,
    graphs: &[GraphSummary],
    rows: &[InventoryRow],
    texts: &BTreeMap<String, TextRecord>,
    missing: &[MissingText],
    unlicensed: &[String],
    first_party: &[FirstPartyRow],
) -> Vec<u8> {
    let mut out = String::new();
    let with_text = rows.iter().filter(|row| row.text_count > 0).count();
    let with_local_text = rows
        .iter()
        .filter(|row| row.text_source == Some(TextSource::Local))
        .count();
    let with_spdx_text = rows
        .iter()
        .filter(|row| row.text_source == Some(TextSource::Spdx))
        .count();
    let _ = writeln!(out, "# Third-party licenses\n");
    let _ = writeln!(
        out,
        "<!-- generated-by: {GENERATOR_ID}/{GENERATOR_VERSION} -->"
    );
    let _ = writeln!(out, "<!-- source-commit: {} -->", request.source_commit);
    let _ = writeln!(
        out,
        "<!-- source-date-epoch: {} -->",
        request.source_date_epoch
    );
    let _ = writeln!(
        out,
        "<!-- source-date: {} -->",
        rfc3339_from_unix(request.source_date_epoch)
    );
    let _ = writeln!(
        out,
        "<!-- text-coverage: third-party={} local={} spdx={} unlicensed={} missing={} -->",
        rows.len(),
        with_local_text,
        with_spdx_text,
        unlicensed.len(),
        rows.len() - with_text
    );
    let _ = writeln!(
        out,
        "<!-- dependency-graph: cargo metadata --format-version 1 --locked --offline -->\n"
    );
    let _ = writeln!(
        out,
        "Generated by `scripts/build-release.sh` from the Cargo dependency graph"
    );
    let _ = writeln!(
        out,
        "recorded in `Cargo.lock`. Crates are every package reachable through normal"
    );
    let _ = writeln!(
        out,
        "dependency edges from the packaged Rust binaries and the Zed extension wasm."
    );
    let _ = writeln!(
        out,
        "`cargo metadata` resolves optional crates present in `Cargo.lock` even when a"
    );
    let _ = writeln!(
        out,
        "feature set does not activate them, so this inventory is a superset of the"
    );
    let _ = writeln!(out, "linked graph and never omits a possible dependency.");
    let _ = writeln!(
        out,
        "License texts are read from the local Cargo package directories. When a"
    );
    let _ = writeln!(
        out,
        "package ships no license file, a canonical SPDX text vendored with the"
    );
    let _ = writeln!(
        out,
        "generator is used and marked `spdx`; no network access is used. This is a"
    );
    let _ = writeln!(
        out,
        "build-time inventory, not a legal review, and it does not choose between"
    );
    let _ = writeln!(out, "offered license alternatives.\n");

    let _ = writeln!(out, "## Dependency graphs\n");
    let _ = writeln!(
        out,
        "| workspace | target | root packages | third-party crates |"
    );
    let _ = writeln!(out, "| --- | --- | --- | ---: |");
    for graph in graphs {
        let packages = graph
            .packages
            .iter()
            .map(|name| format!("`{}`", cell(name)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "| {} | `{}` | {} | {} |",
            cell(&graph.label),
            cell(&graph.target),
            packages,
            graph.third_party_crates
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Rust dependency inventory\n");
    let _ = writeln!(
        out,
        "Third-party crates: {}. With license text: {} (local {}, vendored SPDX {}).",
        rows.len(),
        with_text,
        with_local_text,
        with_spdx_text
    );
    let _ = writeln!(
        out,
        "Text unavailable: {}. Unlicensed (local-only override): {}.\n",
        rows.len() - with_text,
        unlicensed.len()
    );
    let _ = writeln!(
        out,
        "| crate | version | license expression | workspace | license text | text source |"
    );
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- |");
    for row in rows {
        let expression = if row.expressions.is_empty() {
            "`<none>`".to_owned()
        } else {
            row.expressions
                .iter()
                .map(|expression| format!("`{}`", cell(expression)))
                .collect::<Vec<_>>()
                .join("; ")
        };
        let workspaces = row
            .workspaces
            .iter()
            .map(|workspace| cell(workspace))
            .collect::<Vec<_>>()
            .join(", ");
        let text = if row.text_count > 0 {
            format!("{} file(s)", row.text_count)
        } else if row.expressions.is_empty() {
            "**unlicensed (local-only override)**".to_owned()
        } else {
            "unavailable in local cache".to_owned()
        };
        let text_source = row.text_source.map_or("none", TextSource::label);
        let _ = writeln!(
            out,
            "| `{}` | `{}` | {} | {} | {} | {} |",
            cell(&row.name),
            cell(&row.version),
            expression,
            workspaces,
            text,
            text_source
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## First-party packages\n");
    let _ = writeln!(
        out,
        "Packages built from this repository are not third-party dependencies."
    );
    let _ = writeln!(
        out,
        "Their own license is a separate owner decision: the repository currently"
    );
    let _ = writeln!(
        out,
        "declares no `license`/`license-file` in Cargo metadata and contains no"
    );
    let _ = writeln!(
        out,
        "first-party `LICENSE` file, so the packaged artifacts carry no first-party"
    );
    let _ = writeln!(out, "license terms.\n");
    let blocked = first_party
        .iter()
        .any(|record| record.expressions.is_empty() && !record.has_text);
    if blocked {
        let _ = writeln!(out, "- first-party-license: BLOCKED owner decision");
    }
    for record in first_party {
        let declared = if record.expressions.is_empty() {
            if record.has_text {
                "no license expression; a license text file is present".to_owned()
            } else {
                "no license expression and no license file".to_owned()
            }
        } else {
            record
                .expressions
                .iter()
                .map(|expression| format!("`{}`", cell(expression)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            out,
            "- `{}` `{}`: {declared}",
            cell(&record.name),
            cell(&record.version)
        );
    }
    if !blocked {
        let _ = writeln!(out, "- first-party-license: declared in Cargo metadata");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## License texts\n");
    let _ = writeln!(
        out,
        "Each byte-identical license text is included once. \"Applies to\" lists"
    );
    let _ = writeln!(
        out,
        "every crate that depends on that exact text. A `local` text was copied from"
    );
    let _ = writeln!(
        out,
        "the crate's package directory in the local Cargo cache. An `spdx` text is a"
    );
    let _ = writeln!(
        out,
        "canonical SPDX text vendored with the generator and provided under the"
    );
    let _ = writeln!(
        out,
        "crate's recorded license expression; it is not a verbatim copy of an"
    );
    let _ = writeln!(
        out,
        "upstream file. No crate-specific copyright line is fabricated: canonical"
    );
    let _ = writeln!(
        out,
        "license text, copyright notices remain with upstream distribution.\n"
    );
    let mut ordered_texts: Vec<&TextRecord> = texts.values().collect();
    ordered_texts.sort_by(|left, right| left.label.cmp(&right.label));
    for text in ordered_texts {
        let _ = writeln!(out, "### `{}`\n", cell(&text.label));
        match (text.source, text.spdx_id) {
            (TextSource::Spdx, Some(id)) => {
                let _ = writeln!(
                    out,
                    "Source: `spdx` canonical `{id}` text vendored at `assets/spdx/{}`;",
                    cell(text.label.strip_prefix("spdx/").unwrap_or(&text.label))
                );
                let _ = writeln!(
                    out,
                    "provided under the crate's license expression, not copied from the"
                );
                let _ = writeln!(
                    out,
                    "upstream package. Canonical license text, copyright notices remain with upstream distribution.\n"
                );
            }
            _ => {
                let _ = writeln!(out, "Source: `local` package directory text.\n");
            }
        }
        let _ = writeln!(
            out,
            "Applies to: {}\n",
            text.applies_to
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        let fence = fence_for(&text.bytes);
        let _ = writeln!(out, "{fence}text");
        out.push_str(&String::from_utf8_lossy(&text.bytes));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        let _ = writeln!(out, "{fence}\n");
    }

    let _ = writeln!(out, "## License texts unavailable in the local cache\n");
    if missing.is_empty() {
        let _ = writeln!(
            out,
            "None. Every third-party crate has at least one local or vendored SPDX license text.\n"
        );
    } else {
        let _ = writeln!(
            out,
            "These crates declare a license expression but no `LICENSE*`, `NOTICE*`,"
        );
        let _ = writeln!(
            out,
            "`COPYING*` or `UNLICENSE*` file is present in the local Cargo cache, and no"
        );
        let _ = writeln!(
            out,
            "vendored SPDX text matches the recorded expression. The expression is"
        );
        let _ = writeln!(
            out,
            "recorded, but the upstream text must be obtained or a canonical text"
        );
        let _ = writeln!(
            out,
            "vendored before publication instead of being treated as included.\n"
        );
        for entry in missing {
            let expression = entry.expressions.join("; ");
            let _ = writeln!(
                out,
                "- `{}` `{}` (`{}`): license text unavailable in local cache",
                cell(&entry.name),
                cell(&entry.version),
                cell(&expression)
            );
        }
        let _ = writeln!(out);
    }

    if !unlicensed.is_empty() {
        let _ = writeln!(
            out,
            "## Unlicensed crates accepted with `ALLOW_UNLICENSED=1`\n"
        );
        let _ = writeln!(
            out,
            "This inventory is local-only and must not be published: the following"
        );
        let _ = writeln!(
            out,
            "crates have neither a license expression nor a local license text.\n"
        );
        for entry in unlicensed {
            let _ = writeln!(out, "- {entry}");
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "## Reproducibility\n");
    let _ = writeln!(out, "- Generator: `{GENERATOR_ID}/{GENERATOR_VERSION}`");
    let _ = writeln!(out, "- Source commit: `{}`", request.source_commit);
    let _ = writeln!(
        out,
        "- `SOURCE_DATE_EPOCH`: `{}` (`{}`)",
        request.source_date_epoch,
        rfc3339_from_unix(request.source_date_epoch)
    );
    let _ = writeln!(
        out,
        "- Dependency graph: `cargo metadata --format-version 1 --locked --offline`"
    );
    let _ = writeln!(
        out,
        "- License texts: local Cargo package directories, then canonical SPDX texts"
    );
    let _ = writeln!(
        out,
        "  vendored at `crates/wxml-delivery/assets/spdx/`; no network access"
    );
    if request.allow_unlicensed {
        let _ = writeln!(
            out,
            "- **Warning**: `ALLOW_UNLICENSED=1` was set; this inventory is not a release artifact."
        );
    }
    out.into_bytes()
}

fn cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
        .replace('`', "'")
}

/// Returns a `~` code fence longer than any run of tildes in the text.
fn fence_for(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut longest = 0_usize;
    let mut current = 0_usize;
    for character in text.chars() {
        if character == '~' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "~".repeat(longest.max(2) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let base = std::env::var_os("WXML_TEST_TMPDIR")
                .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
            let path = base.join(format!(
                "wxml-licenses-{name}-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct FixturePackage {
        name: &'static str,
        version: &'static str,
        license: Option<&'static str>,
        files: Vec<(&'static str, &'static [u8])>,
        third_party: bool,
    }

    fn write_metadata(base: &Path, roots: &[&str], packages: &[FixturePackage]) -> PathBuf {
        let mut values = Vec::new();
        let mut nodes = Vec::new();
        for package in packages {
            let directory = base.join(format!("{}-{}", package.name, package.version));
            std::fs::create_dir_all(&directory).unwrap();
            for (name, bytes) in &package.files {
                std::fs::write(directory.join(name), bytes).unwrap();
            }
            let id = format!(
                "{}+{}-{}",
                if package.third_party {
                    "registry+https://github.com/rust-lang/crates.io-index"
                } else {
                    "path+file:///workspace"
                },
                package.name,
                package.version
            );
            values.push(serde_json::json!({
                "id": id,
                "name": package.name,
                "version": package.version,
                "license": package.license,
                "license_file": serde_json::Value::Null,
                "manifest_path": directory.join("Cargo.toml"),
                "source": if package.third_party {
                    serde_json::Value::String("registry+https://github.com/rust-lang/crates.io-index".to_owned())
                } else {
                    serde_json::Value::Null
                },
            }));
            nodes.push(serde_json::json!({"id": id, "deps": []}));
        }
        // Build a chain so every package except the root is reachable.
        for index in 1..nodes.len() {
            let pkg = nodes[index]["id"].clone();
            nodes[index - 1]["deps"] = serde_json::json!([{
                "pkg": pkg,
                "dep_kinds": [{"kind": serde_json::Value::Null}]
            }]);
        }
        let root_ids: Vec<serde_json::Value> = values
            .iter()
            .filter(|value| roots.contains(&value["name"].as_str().unwrap_or_default()))
            .map(|value| value["id"].clone())
            .collect();
        assert_eq!(root_ids.len(), roots.len());
        let document = serde_json::json!({
            "packages": values,
            "resolve": {"nodes": nodes},
            "workspace_members": root_ids,
        });
        let path = base.join("metadata.json");
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        path
    }

    fn request(metadata: PathBuf, roots: &[&str]) -> LicenseRequest {
        LicenseRequest {
            roots: vec![MetadataRoot {
                metadata,
                label: "root workspace".to_owned(),
                target: "x86_64-unknown-linux-gnu".to_owned(),
                packages: roots.iter().map(|name| (*name).to_owned()).collect(),
            }],
            source_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            source_date_epoch: 1_763_683_200,
            allow_unlicensed: false,
        }
    }

    #[test]
    fn renders_deterministically_and_deduplicates_license_texts() {
        let base = TempDir::new("deterministic");
        let shared = b"MIT License\n\nPermission is hereby granted.\n";
        let metadata = write_metadata(
            base.path(),
            &["app"],
            &[
                FixturePackage {
                    name: "app",
                    version: "0.1.0",
                    license: None,
                    files: vec![],
                    third_party: false,
                },
                FixturePackage {
                    name: "alpha",
                    version: "1.0.0",
                    license: Some("MIT OR Apache-2.0"),
                    files: vec![("LICENSE", shared), ("NOTICE", b"alpha notice\n")],
                    third_party: true,
                },
                FixturePackage {
                    name: "beta",
                    version: "2.0.0",
                    license: Some("MIT"),
                    files: vec![("LICENSE", shared)],
                    third_party: true,
                },
            ],
        );
        let first = generate(&request(metadata.clone(), &["app"])).unwrap();
        let second = generate(&request(metadata, &["app"])).unwrap();
        assert_eq!(first.0, second.0);
        let text = String::from_utf8(first.0).unwrap();
        assert_eq!(first.1.third_party_crates, 2);
        assert_eq!(first.1.crates_with_text, 2);
        assert_eq!(first.1.crates_with_local_text, 2);
        assert_eq!(first.1.crates_with_spdx_text, 0);
        assert_eq!(first.1.distinct_texts, 2);
        assert_eq!(
            text.matches("Permission is hereby granted").count(),
            1,
            "identical license text must be emitted once"
        );
        assert!(text.contains("first-party-license: BLOCKED owner decision"));
        assert!(text.contains("<!-- source-date-epoch: 1763683200 -->"));
        assert!(text.find("`alpha`").unwrap() < text.find("`beta`").unwrap());
    }

    #[test]
    fn records_missing_local_and_vendored_text_instead_of_omitting_the_crate() {
        let base = TempDir::new("missing-text");
        let metadata = write_metadata(
            base.path(),
            &["app"],
            &[
                FixturePackage {
                    name: "app",
                    version: "0.1.0",
                    license: None,
                    files: vec![],
                    third_party: false,
                },
                FixturePackage {
                    name: "no-text",
                    version: "3.0.0",
                    license: Some("LicenseRef-Proprietary"),
                    files: vec![],
                    third_party: true,
                },
            ],
        );
        let (bytes, summary) = generate(&request(metadata, &["app"])).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(summary.crates_without_text, 1);
        assert_eq!(summary.crates_with_text, 0);
        assert_eq!(summary.crates_with_local_text, 0);
        assert_eq!(summary.crates_with_spdx_text, 0);
        assert!(text.contains("`no-text`"));
        assert!(text.contains("license text unavailable in local cache"));
        assert!(text.contains("missing=1"), "{text}");
    }

    #[test]
    fn vendored_spdx_texts_cover_the_recorded_expression_shapes() {
        let dual = vendored_license_texts("MIT OR Apache-2.0");
        assert_eq!(dual.len(), 2, "both alternatives must be included");
        assert!(dual.iter().all(|text| text.source == TextSource::Spdx));
        assert!(dual.iter().any(|text| text.spdx_id == Some("MIT")));
        assert!(dual.iter().any(|text| text.spdx_id == Some("Apache-2.0")));

        let wasm = vendored_license_texts("Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT");
        assert_eq!(wasm.len(), 3, "the exception and both alternatives appear");
        assert!(
            wasm.iter()
                .any(|text| text.spdx_id == Some("LLVM-exception"))
        );
        for text in &wasm {
            let header = String::from_utf8_lossy(&text.bytes);
            assert!(
                header.starts_with("Apache License")
                    || header.starts_with("MIT License")
                    || header.starts_with("---- LLVM Exceptions"),
                "unexpected vendored text header for {:?}",
                text.spdx_id
            );
        }

        assert_eq!(vendored_license_texts("Unicode-3.0").len(), 1);
        assert_eq!(vendored_license_texts("Zlib").len(), 1);
        assert!(vendored_license_texts("GPL-3.0-only").is_empty());
        assert!(vendored_license_texts("").is_empty());
        assert!(vendored_text("apache-2.0").is_some());
    }

    #[test]
    fn falls_back_to_vendored_spdx_text_and_records_the_source() {
        let base = TempDir::new("spdx-fallback");
        let metadata = write_metadata(
            base.path(),
            &["app"],
            &[
                FixturePackage {
                    name: "app",
                    version: "0.1.0",
                    license: None,
                    files: vec![],
                    third_party: false,
                },
                FixturePackage {
                    name: "auditable-serde",
                    version: "0.8.0",
                    license: Some("MIT OR Apache-2.0"),
                    files: vec![],
                    third_party: true,
                },
            ],
        );
        let (bytes, summary) = generate(&request(metadata, &["app"])).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(summary.crates_with_text, 1);
        assert_eq!(summary.crates_with_local_text, 0);
        assert_eq!(summary.crates_with_spdx_text, 1);
        assert_eq!(summary.crates_without_text, 0);
        assert!(text.contains("| `auditable-serde` | `0.8.0` | `MIT OR Apache-2.0`"));
        assert!(text.contains("| spdx |"));
        assert!(text.contains("### `spdx/Apache-2.0.txt`"));
        assert!(text.contains("### `spdx/MIT.txt`"));
        assert!(
            text.contains("Canonical license text, copyright notices remain with upstream"),
            "the fallback must state where notice obligations remain"
        );
        assert!(
            text.contains("local=0 spdx=1 unlicensed=0 missing=0"),
            "{text}"
        );
        assert!(
            !text.contains("license text unavailable in local cache"),
            "a vendored text is not a missing text"
        );
    }

    #[test]
    fn prefers_local_text_over_vendored_spdx_text() {
        let base = TempDir::new("local-wins");
        let metadata = write_metadata(
            base.path(),
            &["app"],
            &[
                FixturePackage {
                    name: "app",
                    version: "0.1.0",
                    license: None,
                    files: vec![],
                    third_party: false,
                },
                FixturePackage {
                    name: "has-text",
                    version: "1.2.3",
                    license: Some("MIT"),
                    files: vec![("LICENSE", b"local MIT text\n")],
                    third_party: true,
                },
            ],
        );
        let (bytes, summary) = generate(&request(metadata, &["app"])).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(summary.crates_with_local_text, 1);
        assert_eq!(summary.crates_with_spdx_text, 0);
        assert!(text.contains("| local |"));
        assert!(!text.contains("### `spdx/MIT.txt`"));
        assert!(text.contains("Source: `local` package directory text."));
    }

    #[test]
    fn every_zed_wasm_crate_expression_gets_a_vendored_text() {
        let base = TempDir::new("zed-wasm-coverage");
        let expression = "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT";
        let crates = [
            ("auditable-serde", "0.8.0", "MIT OR Apache-2.0"),
            ("wasm-encoder", "0.227.1", expression),
            ("wasm-metadata", "0.227.1", expression),
            ("wasmparser", "0.227.1", expression),
            ("wit-bindgen", "0.41.0", expression),
            ("wit-bindgen-core", "0.41.0", expression),
            ("wit-bindgen-rt", "0.41.0", expression),
            ("wit-bindgen-rust", "0.41.0", expression),
            ("wit-bindgen-rust-macro", "0.41.0", expression),
            ("wit-component", "0.227.1", expression),
            ("wit-parser", "0.227.1", expression),
        ];
        let mut packages = vec![FixturePackage {
            name: "app",
            version: "0.1.0",
            license: None,
            files: vec![],
            third_party: false,
        }];
        for (name, version, license) in crates {
            packages.push(FixturePackage {
                name,
                version,
                license: Some(license),
                files: vec![],
                third_party: true,
            });
        }
        let metadata = write_metadata(base.path(), &["app"], &packages);
        let (bytes, summary) = generate(&request(metadata, &["app"])).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(summary.third_party_crates, 11);
        assert_eq!(summary.crates_with_text, 11);
        assert_eq!(summary.crates_with_local_text, 0);
        assert_eq!(summary.crates_with_spdx_text, 11);
        assert_eq!(summary.crates_without_text, 0);
        assert_eq!(summary.unlicensed_crates, 0);
        assert_eq!(
            text.matches("| spdx |").count(),
            11,
            "every crate row must record text_source=spdx"
        );
        for (name, version, _) in crates {
            assert!(
                text.contains(&format!("| `{name}` | `{version}` |")),
                "inventory is missing {name} {version}"
            );
        }
        assert!(text.contains("local=0 spdx=11 unlicensed=0 missing=0"));
    }

    #[test]
    fn fails_closed_when_a_crate_has_no_expression_and_no_text() {
        let base = TempDir::new("unlicensed");
        let metadata = write_metadata(
            base.path(),
            &["app"],
            &[
                FixturePackage {
                    name: "app",
                    version: "0.1.0",
                    license: None,
                    files: vec![],
                    third_party: false,
                },
                FixturePackage {
                    name: "unlicensed",
                    version: "4.0.0",
                    license: None,
                    files: vec![],
                    third_party: true,
                },
            ],
        );
        let error = generate(&request(metadata.clone(), &["app"])).unwrap_err();
        assert!(
            error.to_string().contains("no license expression"),
            "{error}"
        );
        let mut allowed = request(metadata, &["app"]);
        allowed.allow_unlicensed = true;
        let (bytes, summary) = generate(&allowed).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(summary.unlicensed_crates, 1);
        assert!(text.contains("ALLOW_UNLICENSED=1"));
        assert!(text.contains("**unlicensed (local-only override)**"));
    }

    #[test]
    fn rejects_a_missing_declared_license_file() {
        let base = TempDir::new("declared-missing");
        let metadata = write_metadata(
            base.path(),
            &["app"],
            &[FixturePackage {
                name: "app",
                version: "0.1.0",
                license: None,
                files: vec![],
                third_party: false,
            }],
        );
        // Rewrite the metadata so `app` declares a license file that is absent.
        let text = std::fs::read_to_string(&metadata).unwrap();
        let text = text.replacen(
            "\"license_file\":null",
            "\"license_file\":\"MISSING-LICENSE\"",
            1,
        );
        std::fs::write(&metadata, text).unwrap();
        let error = generate(&request(metadata, &["app"])).unwrap_err();
        assert!(
            error.to_string().contains("declares license file"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_dependency_graph_without_roots() {
        let base = TempDir::new("no-roots");
        let metadata = write_metadata(
            base.path(),
            &["app"],
            &[FixturePackage {
                name: "app",
                version: "0.1.0",
                license: None,
                files: vec![],
                third_party: false,
            }],
        );
        let mut broken = request(metadata, &["app"]);
        broken.roots[0].packages.clear();
        let error = generate(&broken).unwrap_err();
        assert!(error.to_string().contains("no root packages"), "{error}");
    }
}
