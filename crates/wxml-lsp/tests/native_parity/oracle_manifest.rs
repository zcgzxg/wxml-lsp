//! Frozen legacy oracle manifest gate (P7-2).
//!
//! `docs/plans/tsgo-legacy-oracle-manifest.json` archives the pre-P7
//! TypeScript oracle: the 21 test-file hashes/line counts, the 339
//! `test(...)`/`it(...)` body hashes with their file/line anchors, the
//! `support.ts` hash, and the four fixture-tree hashes. It is generated while
//! `backends/typescript/test` still exists and stays authoritative after that
//! tree is deleted.
//!
//! The manifest is not a second ledger. Every record is cross-checked against
//! `docs/plans/tsgo-test-ledger.json`, and the canonical projection is pinned
//! by [`FROZEN_MANIFEST_SHA256`]. Editing the manifest therefore requires a
//! deliberate change in two places; a count, file hash, body hash, or missing
//! field is a hard failure, never a silent downgrade to "no verification".
//!
//! Source modes:
//! - live: `backends/typescript/test` exists. Every file and case is
//!   recomputed from the bytes with [`crate::source_scan`], and the result must
//!   equal both the ledger and the manifest.
//! - frozen: the legacy tree was deleted. The manifest records are verified
//!   against the ledger and against each other; line counts stay frozen and
//!   every case line remains bounded by its file's frozen line count.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::source_scan;

/// Repository-relative manifest path.
pub(crate) const ORACLE_MANIFEST_RELATIVE_PATH: &str =
    "docs/plans/tsgo-legacy-oracle-manifest.json";
/// Legacy TypeScript test root, relative to the workspace.
pub(crate) const LEGACY_TEST_SOURCE_ROOT: &str = "backends/typescript/test";
/// Frozen manifest schema.
pub(crate) const MANIFEST_SCHEMA: &str = "wxml-tsgo-legacy-oracle-manifest/1";
/// HEAD at which the manifest was generated. This is historical evidence, not
/// the current checkout; asserting it prevents a silent regeneration.
pub(crate) const ORACLE_BASELINE_HEAD: &str = "807a1578173992ad16f15e22b45f8dd62b024df3";
/// SHA-256 of the canonical projection. The manifest is frozen at this digest;
/// a change requires editing this constant deliberately.
pub(crate) const FROZEN_MANIFEST_SHA256: &str =
    "14cd870e8ce7d15294285fe9205bb98bb121164e44f8c67ee3c433aa43508123";

const MANIFEST_TOP_LEVEL_KEYS: &[&str] = &[
    "schema",
    "generated_at",
    "baseline",
    "algorithm",
    "counts",
    "support_file",
    "files",
    "fixture_trees",
    "cases",
    "projection_sha256",
];
const BASELINE_KEYS: &[&str] = &["head", "branch", "source_root", "ledger_schema"];
const ALGORITHM_KEYS: &[&str] = &[
    "name",
    "path_order",
    "case_body",
    "file_hash",
    "line_count",
    "tree",
    "symlinks",
];
const COUNT_KEYS: &[&str] = &["test_files", "support_files", "cases", "fixture_trees"];
const SUPPORT_KEYS: &[&str] = &["path", "sha256", "lines"];
const FILE_KEYS: &[&str] = &[
    "file",
    "sha256",
    "lines",
    "test_declarations",
    "literal_declarations",
    "dynamic_declarations",
    "registrations",
];
const TREE_KEYS: &[&str] = &["tree", "files", "bytes", "sha256", "archived_ok"];
const CASE_KEYS: &[&str] = &["case_id", "file", "line", "body_sha256"];
const LEDGER_SCHEMA: &str = "wxml-tsgo-test-ledger/1";

/// Frozen `support.ts` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestSupport {
    /// Repository-relative path.
    pub(crate) path: String,
    /// Lowercase SHA-256 of the raw file bytes.
    pub(crate) sha256: String,
    /// Frozen line count.
    pub(crate) lines: u64,
}

/// One frozen TypeScript test file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestFile {
    /// Repository-relative path.
    pub(crate) file: String,
    /// Lowercase SHA-256 of the raw file bytes.
    pub(crate) sha256: String,
    /// Frozen line count.
    pub(crate) lines: u64,
    /// Static `test(...)` declarations.
    pub(crate) test_declarations: u64,
    /// Literal declarations.
    pub(crate) literal_declarations: u64,
    /// Dynamic declarations.
    pub(crate) dynamic_declarations: u64,
    /// Expanded registrations.
    pub(crate) registrations: u64,
}

/// One frozen fixture tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestTree {
    /// Repository-relative tree path.
    pub(crate) tree: String,
    /// Regular file count.
    pub(crate) files: u64,
    /// Total raw bytes.
    pub(crate) bytes: u64,
    /// v1 tree hash.
    pub(crate) sha256: String,
    /// True when the tree may be absent after P7 deletion.
    pub(crate) archived_ok: bool,
}

/// One frozen case anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestCase {
    /// Stable ledger case ID.
    pub(crate) case_id: String,
    /// Repository-relative source file.
    pub(crate) file: String,
    /// 1-based declaration line.
    pub(crate) line: u64,
    /// SHA-256 of the exact `test(...)` `CallExpression` text.
    pub(crate) body_sha256: String,
}

/// Parsed and projection-checked legacy oracle manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OracleManifest {
    /// Manifest schema.
    pub(crate) schema: String,
    /// Generation timestamp.
    pub(crate) generated_at: String,
    /// Generation HEAD.
    pub(crate) head: String,
    /// Generation branch.
    pub(crate) branch: String,
    /// Legacy source root.
    pub(crate) source_root: String,
    /// Ledger schema in force at generation time.
    pub(crate) ledger_schema: String,
    /// Frozen `support.ts`.
    pub(crate) support: ManifestSupport,
    /// Frozen test files, sorted by path.
    pub(crate) files: Vec<ManifestFile>,
    /// Frozen fixture trees, sorted by path.
    pub(crate) trees: Vec<ManifestTree>,
    /// Frozen cases, sorted by `case_id`.
    pub(crate) cases: Vec<ManifestCase>,
    /// Canonical projection digest.
    pub(crate) projection_sha256: String,
}

/// Which source authority the gate used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacySourceMode {
    /// Bytes under `backends/typescript/test` were re-hashed.
    Live,
    /// The legacy tree is gone; the frozen manifest was checked against the ledger.
    Frozen,
}

/// Summary of a successful legacy oracle check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LegacyOracleReport {
    /// Source authority used.
    pub(crate) mode: LegacySourceMode,
    /// Number of test files checked.
    pub(crate) files: usize,
    /// Number of cases checked.
    pub(crate) cases: usize,
}

/// Loads the frozen manifest from the workspace and verifies the hard pins.
///
/// A missing or invalid manifest is always a failure; there is no skip path.
pub(crate) fn load_manifest(workspace: &Path) -> OracleManifest {
    let path = workspace.join(ORACLE_MANIFEST_RELATIVE_PATH);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "legacy oracle manifest is required but missing/unreadable at {}: {error}",
            path.display()
        )
    });
    parse_manifest(
        &text,
        &path.display().to_string(),
        Some(FROZEN_MANIFEST_SHA256),
        Some(ORACLE_BASELINE_HEAD),
    )
    .unwrap_or_else(|error| {
        panic!(
            "legacy oracle manifest {} is invalid: {error}",
            path.display()
        )
    })
}

/// Parses and validates a manifest, optionally enforcing the frozen pins.
pub(crate) fn parse_manifest(
    text: &str,
    context: &str,
    pinned_projection: Option<&str>,
    pinned_head: Option<&str>,
) -> Result<OracleManifest, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("{context} is not valid JSON: {error}"))?;
    let root = value
        .as_object()
        .ok_or_else(|| format!("{context} root must be a JSON object"))?;
    require_exact_keys(root, MANIFEST_TOP_LEVEL_KEYS, context)?;

    let mut manifest = parse_parts(root, context)?;
    let stored = required_str(root, "projection_sha256", context)?;
    if !is_lowercase_sha256(stored) {
        return Err(format!(
            "{context}.projection_sha256 must be lowercase SHA-256 hex"
        ));
    }
    let computed = canonical_projection_sha256(&manifest);
    if stored != computed {
        return Err(format!(
            "{context}.projection_sha256 does not match the canonical projection; \
             the manifest was edited without a deliberate digest update"
        ));
    }
    if let Some(pin) = pinned_projection {
        if computed != pin {
            return Err(format!(
                "{context}.projection_sha256 {computed} does not match the frozen pin {pin}"
            ));
        }
    }
    if let Some(head) = pinned_head {
        if manifest.head != head {
            return Err(format!(
                "{context}.baseline.head is {}, expected the frozen {head}",
                manifest.head
            ));
        }
    }
    manifest.projection_sha256 = computed;
    Ok(manifest)
}

fn parse_parts(root: &Map<String, Value>, context: &str) -> Result<OracleManifest, String> {
    let schema = required_str(root, "schema", context)?;
    if schema != MANIFEST_SCHEMA {
        return Err(format!(
            "{context}.schema is {schema:?}, expected {MANIFEST_SCHEMA:?}"
        ));
    }
    let generated_at = required_str(root, "generated_at", context)?;
    if generated_at.is_empty() {
        return Err(format!("{context}.generated_at must not be empty"));
    }

    let baseline = required_object(root, "baseline", context)?;
    require_exact_keys(baseline, BASELINE_KEYS, &format!("{context}.baseline"))?;
    let head = required_str(baseline, "head", &format!("{context}.baseline"))?;
    if head.len() != 40 || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "{context}.baseline.head must be a 40-character git object id"
        ));
    }
    let branch = required_str(baseline, "branch", &format!("{context}.baseline"))?;
    if branch.is_empty() {
        return Err(format!("{context}.baseline.branch must not be empty"));
    }
    let source_root = required_str(baseline, "source_root", &format!("{context}.baseline"))?;
    if source_root != LEGACY_TEST_SOURCE_ROOT {
        return Err(format!(
            "{context}.baseline.source_root is {source_root:?}, \
             expected {LEGACY_TEST_SOURCE_ROOT:?}"
        ));
    }
    let ledger_schema = required_str(baseline, "ledger_schema", &format!("{context}.baseline"))?;
    if ledger_schema != LEDGER_SCHEMA {
        return Err(format!(
            "{context}.baseline.ledger_schema is {ledger_schema:?}, expected {LEDGER_SCHEMA:?}"
        ));
    }

    let algorithm = required_object(root, "algorithm", context)?;
    require_exact_keys(algorithm, ALGORITHM_KEYS, &format!("{context}.algorithm"))?;
    let algorithm_name = required_str(algorithm, "name", &format!("{context}.algorithm"))?;
    if algorithm_name != "v1" {
        return Err(format!(
            "{context}.algorithm.name is {algorithm_name:?}, expected \"v1\""
        ));
    }
    if required_u64(algorithm, "symlinks", &format!("{context}.algorithm"))? != 0 {
        return Err(format!("{context}.algorithm.symlinks must be 0"));
    }
    for key in ["path_order", "case_body", "file_hash", "line_count", "tree"] {
        if required_str(algorithm, key, &format!("{context}.algorithm"))?.is_empty() {
            return Err(format!("{context}.algorithm.{key} must not be empty"));
        }
    }

    let files = parse_files(root, source_root, context)?;
    let trees = parse_trees(root, context)?;
    let support = parse_support(root, source_root, context)?;
    let cases = parse_cases(root, &files, context)?;

    let counts = required_object(root, "counts", context)?;
    require_exact_keys(counts, COUNT_KEYS, &format!("{context}.counts"))?;
    let declared_files = required_u64(counts, "test_files", &format!("{context}.counts"))?;
    let declared_support = required_u64(counts, "support_files", &format!("{context}.counts"))?;
    let declared_cases = required_u64(counts, "cases", &format!("{context}.counts"))?;
    let declared_trees = required_u64(counts, "fixture_trees", &format!("{context}.counts"))?;
    if declared_support != 1 {
        return Err(format!("{context}.counts.support_files must be exactly 1"));
    }
    if declared_files
        != u64::try_from(files.len()).map_err(|_| "file count overflowed u64".to_owned())?
    {
        return Err(format!(
            "{context}.counts.test_files is {declared_files}, but the inventory has {}",
            files.len()
        ));
    }
    if declared_cases
        != u64::try_from(cases.len()).map_err(|_| "case count overflowed u64".to_owned())?
    {
        return Err(format!(
            "{context}.counts.cases is {declared_cases}, but the inventory has {}",
            cases.len()
        ));
    }
    if declared_trees
        != u64::try_from(trees.len()).map_err(|_| "tree count overflowed u64".to_owned())?
    {
        return Err(format!(
            "{context}.counts.fixture_trees is {declared_trees}, but the inventory has {}",
            trees.len()
        ));
    }

    Ok(OracleManifest {
        schema: schema.to_owned(),
        generated_at: generated_at.to_owned(),
        head: head.to_owned(),
        branch: branch.to_owned(),
        source_root: source_root.to_owned(),
        ledger_schema: ledger_schema.to_owned(),
        support,
        files,
        trees,
        cases,
        projection_sha256: String::new(),
    })
}

fn parse_files(
    root: &Map<String, Value>,
    source_root: &str,
    context: &str,
) -> Result<Vec<ManifestFile>, String> {
    let entries = required_array(root, "files", context)?;
    let mut files = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("{context}.files[{index}]");
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("{path} must be a JSON object"))?;
        require_exact_keys(entry, FILE_KEYS, &path)?;
        let file = required_str(entry, "file", &path)?;
        validate_relative(file, &path)?;
        if !file.starts_with(&format!("{source_root}/")) || !file.ends_with(".test.ts") {
            return Err(format!(
                "{path}.file must be a .test.ts under {source_root}: {file}"
            ));
        }
        let sha256 = required_str(entry, "sha256", &path)?;
        if !is_lowercase_sha256(sha256) {
            return Err(format!("{path}.sha256 must be lowercase SHA-256 hex"));
        }
        let lines = required_u64(entry, "lines", &path)?;
        if lines == 0 {
            return Err(format!("{path}.lines must be greater than zero"));
        }
        let test_declarations = required_u64(entry, "test_declarations", &path)?;
        let literal_declarations = required_u64(entry, "literal_declarations", &path)?;
        let dynamic_declarations = required_u64(entry, "dynamic_declarations", &path)?;
        let registrations = required_u64(entry, "registrations", &path)?;
        if test_declarations == 0 {
            return Err(format!(
                "{path}.test_declarations must be greater than zero"
            ));
        }
        if literal_declarations.checked_add(dynamic_declarations) != Some(test_declarations) {
            return Err(format!(
                "{path} literal/dynamic declarations do not sum to test_declarations"
            ));
        }
        if registrations < test_declarations {
            return Err(format!(
                "{path}.registrations must not be less than test_declarations"
            ));
        }
        files.push(ManifestFile {
            file: file.to_owned(),
            sha256: sha256.to_owned(),
            lines,
            test_declarations,
            literal_declarations,
            dynamic_declarations,
            registrations,
        });
    }
    if files.is_empty() {
        return Err(format!("{context}.files must not be empty"));
    }
    require_strict_path_order(
        files.iter().map(|file| file.file.as_str()),
        &format!("{context}.files"),
    )?;
    Ok(files)
}

fn parse_trees(root: &Map<String, Value>, context: &str) -> Result<Vec<ManifestTree>, String> {
    let entries = required_array(root, "fixture_trees", context)?;
    let mut trees = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("{context}.fixture_trees[{index}]");
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("{path} must be a JSON object"))?;
        require_exact_keys(entry, TREE_KEYS, &path)?;
        let tree = required_str(entry, "tree", &path)?;
        validate_relative(tree, &path)?;
        let files = required_u64(entry, "files", &path)?;
        if files == 0 {
            return Err(format!("{path}.files must be greater than zero"));
        }
        let bytes = required_u64(entry, "bytes", &path)?;
        if bytes == 0 {
            return Err(format!("{path}.bytes must be greater than zero"));
        }
        let sha256 = required_str(entry, "sha256", &path)?;
        if !is_lowercase_sha256(sha256) {
            return Err(format!("{path}.sha256 must be lowercase SHA-256 hex"));
        }
        let archived_ok = entry
            .get("archived_ok")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("{path}.archived_ok must be a boolean"))?;
        trees.push(ManifestTree {
            tree: tree.to_owned(),
            files,
            bytes,
            sha256: sha256.to_owned(),
            archived_ok,
        });
    }
    if trees.is_empty() {
        return Err(format!("{context}.fixture_trees must not be empty"));
    }
    require_strict_path_order(
        trees.iter().map(|tree| tree.tree.as_str()),
        &format!("{context}.fixture_trees"),
    )?;
    Ok(trees)
}

fn parse_support(
    root: &Map<String, Value>,
    source_root: &str,
    context: &str,
) -> Result<ManifestSupport, String> {
    let support = required_object(root, "support_file", context)?;
    require_exact_keys(support, SUPPORT_KEYS, &format!("{context}.support_file"))?;
    let path = required_str(support, "path", &format!("{context}.support_file"))?;
    validate_relative(path, &format!("{context}.support_file"))?;
    let expected = format!("{source_root}/support.ts");
    if path != expected {
        return Err(format!(
            "{context}.support_file.path is {path:?}, expected {expected:?}"
        ));
    }
    let sha256 = required_str(support, "sha256", &format!("{context}.support_file"))?;
    if !is_lowercase_sha256(sha256) {
        return Err(format!(
            "{context}.support_file.sha256 must be lowercase SHA-256 hex"
        ));
    }
    let lines = required_u64(support, "lines", &format!("{context}.support_file"))?;
    if lines == 0 {
        return Err(format!(
            "{context}.support_file.lines must be greater than zero"
        ));
    }
    Ok(ManifestSupport {
        path: path.to_owned(),
        sha256: sha256.to_owned(),
        lines,
    })
}

fn parse_cases(
    root: &Map<String, Value>,
    files: &[ManifestFile],
    context: &str,
) -> Result<Vec<ManifestCase>, String> {
    let entries = required_array(root, "cases", context)?;
    let file_lines: BTreeMap<&str, u64> = files
        .iter()
        .map(|file| (file.file.as_str(), file.lines))
        .collect();
    let mut cases = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    let mut referenced_files = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("{context}.cases[{index}]");
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("{path} must be a JSON object"))?;
        require_exact_keys(entry, CASE_KEYS, &path)?;
        let case_id = required_str(entry, "case_id", &path)?;
        if !is_case_id(case_id) {
            return Err(format!("{path}.case_id is not a stable TS-... identifier"));
        }
        if !seen.insert(case_id.to_owned()) {
            return Err(format!(
                "{context}.cases contains duplicate case_id {case_id}"
            ));
        }
        let file = required_str(entry, "file", &path)?;
        let Some(limit) = file_lines.get(file) else {
            return Err(format!(
                "{path}.file {file} is not present in {context}.files"
            ));
        };
        referenced_files.insert(file.to_owned());
        let line = required_u64(entry, "line", &path)?;
        if line == 0 || line > *limit {
            return Err(format!(
                "{path}.line {line} is outside {file} ({limit} frozen lines)"
            ));
        }
        let body_sha256 = required_str(entry, "body_sha256", &path)?;
        if !is_lowercase_sha256(body_sha256) {
            return Err(format!("{path}.body_sha256 must be lowercase SHA-256 hex"));
        }
        cases.push(ManifestCase {
            case_id: case_id.to_owned(),
            file: file.to_owned(),
            line,
            body_sha256: body_sha256.to_owned(),
        });
    }
    if cases.is_empty() {
        return Err(format!("{context}.cases must not be empty"));
    }
    require_strict_path_order(
        cases.iter().map(|case| case.case_id.as_str()),
        &format!("{context}.cases"),
    )?;
    if referenced_files.len() != files.len() {
        let missing: Vec<&str> = files
            .iter()
            .filter(|file| !referenced_files.contains(&file.file))
            .map(|file| file.file.as_str())
            .collect();
        return Err(format!(
            "{context}.cases does not cover every test file; missing: {}",
            missing.join(", ")
        ));
    }
    Ok(cases)
}

/// Returns the canonical projection digest over the semantic manifest fields.
pub(crate) fn canonical_projection_sha256(manifest: &OracleManifest) -> String {
    let mut lines = vec![
        format!("schema\t{}", manifest.schema),
        format!("head\t{}", manifest.head),
        format!("source_root\t{}", manifest.source_root),
        format!(
            "support\t{}\t{}\t{}",
            manifest.support.path, manifest.support.sha256, manifest.support.lines
        ),
    ];
    for file in &manifest.files {
        lines.push(format!(
            "file\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            file.file,
            file.sha256,
            file.lines,
            file.test_declarations,
            file.literal_declarations,
            file.dynamic_declarations,
            file.registrations
        ));
    }
    for tree in &manifest.trees {
        lines.push(format!(
            "tree\t{}\t{}\t{}\t{}\t{}",
            tree.tree,
            tree.files,
            tree.bytes,
            tree.sha256,
            u8::from(tree.archived_ok)
        ));
    }
    for case in &manifest.cases {
        lines.push(format!(
            "case\t{}\t{}\t{}\t{}",
            case.case_id, case.file, case.line, case.body_sha256
        ));
    }
    let payload = format!("{}\n", lines.join("\n"));
    hash_bytes(payload.as_bytes())
}

/// Cross-checks every manifest record against the frozen ledger.
///
/// This is the frozen-mode authority: it never reads the legacy source tree.
pub(crate) fn verify_frozen_projection(
    ledger: &Value,
    manifest: &OracleManifest,
) -> Result<(), String> {
    let root = ledger
        .as_object()
        .ok_or_else(|| "ledger root must be a JSON object".to_owned())?;
    let counts = required_object(root, "counts", "ledger")?;
    let declared_files = required_u64(counts, "test_files", "ledger.counts")?;
    let declared_cases = required_u64(counts, "cases_in_ledger", "ledger.counts")?;
    if declared_files
        != u64::try_from(manifest.files.len())
            .map_err(|_| "manifest file count overflowed u64".to_owned())?
    {
        return Err(format!(
            "ledger.counts.test_files is {declared_files}, but the manifest has {} files",
            manifest.files.len()
        ));
    }
    if declared_cases
        != u64::try_from(manifest.cases.len())
            .map_err(|_| "manifest case count overflowed u64".to_owned())?
    {
        return Err(format!(
            "ledger.counts.cases_in_ledger is {declared_cases}, \
             but the manifest has {} cases",
            manifest.cases.len()
        ));
    }

    let ledger_files = required_array(root, "files", "ledger")?;
    if ledger_files.len() != manifest.files.len() {
        return Err(format!(
            "ledger.files has {} entries, but the manifest has {} files",
            ledger_files.len(),
            manifest.files.len()
        ));
    }
    let mut files_by_path = BTreeMap::new();
    for entry in ledger_files {
        let entry = entry
            .as_object()
            .ok_or_else(|| "ledger.files[] must be a JSON object".to_owned())?;
        let file = required_str(entry, "file", "ledger.files[]")?;
        let record = (
            required_str(entry, "sha256", file)?,
            required_u64(entry, "test_declarations", file)?,
            required_u64(entry, "literal_declarations", file)?,
            required_u64(entry, "dynamic_declarations", file)?,
            required_u64(entry, "registrations", file)?,
        );
        if files_by_path.insert(file.to_owned(), record).is_some() {
            return Err(format!("ledger.files contains duplicate file {file}"));
        }
    }
    for file in &manifest.files {
        let Some((sha256, test_declarations, literal, dynamic, registrations)) =
            files_by_path.get(&file.file)
        else {
            return Err(format!(
                "manifest file {} is missing from ledger.files",
                file.file
            ));
        };
        if *sha256 != file.sha256 {
            return Err(format!(
                "file hash drift for {}: ledger {sha256} != manifest {}",
                file.file, file.sha256
            ));
        }
        if *test_declarations != file.test_declarations
            || *literal != file.literal_declarations
            || *dynamic != file.dynamic_declarations
            || *registrations != file.registrations
        {
            return Err(format!(
                "declaration counts drift for {} between the ledger and manifest",
                file.file
            ));
        }
    }

    let ledger_cases = required_array(root, "cases", "ledger")?;
    if ledger_cases.len() != manifest.cases.len() {
        return Err(format!(
            "ledger.cases has {} entries, but the manifest has {} cases",
            ledger_cases.len(),
            manifest.cases.len()
        ));
    }
    let mut cases_by_id = BTreeMap::new();
    for entry in ledger_cases {
        let entry = entry
            .as_object()
            .ok_or_else(|| "ledger.cases[] must be a JSON object".to_owned())?;
        let case_id = required_str(entry, "case_id", "ledger.cases[]")?;
        let source = required_object(entry, "source", case_id)?;
        let record = (
            required_str(source, "file", case_id)?.to_owned(),
            required_u64(source, "line", case_id)?,
            required_str(source, "body_sha256", case_id)?.to_owned(),
        );
        if cases_by_id.insert(case_id.to_owned(), record).is_some() {
            return Err(format!("ledger.cases contains duplicate case_id {case_id}"));
        }
    }
    for case in &manifest.cases {
        let Some((file, line, body_sha256)) = cases_by_id.get(&case.case_id) else {
            return Err(format!(
                "manifest case {} is missing from ledger.cases",
                case.case_id
            ));
        };
        if file != &case.file || *line != case.line || body_sha256 != &case.body_sha256 {
            return Err(format!(
                "case anchor drift for {}: ledger {}:{} {} != manifest {}:{} {}",
                case.case_id, file, line, body_sha256, case.file, case.line, case.body_sha256
            ));
        }
    }

    let support = required_object(root, "support_file", "ledger")?;
    let support_path = required_str(support, "path", "ledger.support_file")?;
    let support_sha = required_str(support, "sha256", "ledger.support_file")?;
    if support_path != manifest.support.path || support_sha != manifest.support.sha256 {
        return Err(format!(
            "support.ts drift: ledger {} {} != manifest {} {}",
            support_path, support_sha, manifest.support.path, manifest.support.sha256
        ));
    }

    let fixture_trees = required_object(root, "fixture_trees", "ledger")?;
    let ledger_trees = required_array(fixture_trees, "trees", "ledger.fixture_trees")?;
    if ledger_trees.len() != manifest.trees.len() {
        return Err(format!(
            "ledger.fixture_trees.trees has {} entries, but the manifest has {} trees",
            ledger_trees.len(),
            manifest.trees.len()
        ));
    }
    let mut trees_by_path = BTreeMap::new();
    for entry in ledger_trees {
        let entry = entry
            .as_object()
            .ok_or_else(|| "ledger.fixture_trees.trees[] must be a JSON object".to_owned())?;
        let tree = required_str(entry, "tree", "ledger.fixture_trees.trees[]")?;
        let record = (
            required_u64(entry, "files", tree)?,
            required_u64(entry, "bytes", tree)?,
            required_str(entry, "sha256", tree)?,
        );
        if trees_by_path.insert(tree.to_owned(), record).is_some() {
            return Err(format!(
                "ledger.fixture_trees contains duplicate tree {tree}"
            ));
        }
    }
    for tree in &manifest.trees {
        let Some((files, bytes, sha256)) = trees_by_path.get(&tree.tree) else {
            return Err(format!(
                "manifest tree {} is missing from ledger.fixture_trees",
                tree.tree
            ));
        };
        if *files != tree.files || *bytes != tree.bytes || *sha256 != tree.sha256 {
            return Err(format!(
                "fixture tree drift for {} between the ledger and manifest",
                tree.tree
            ));
        }
    }

    Ok(())
}

/// Verifies the legacy oracle against the live tree when it exists, otherwise
/// against the frozen manifest.
pub(crate) fn verify_legacy_oracle(
    ledger: &Value,
    manifest: &OracleManifest,
    live_root: &Path,
) -> Result<LegacyOracleReport, String> {
    verify_frozen_projection(ledger, manifest)?;
    match fs::symlink_metadata(live_root) {
        Ok(_) => verify_live_source(ledger, manifest, live_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LegacyOracleReport {
            mode: LegacySourceMode::Frozen,
            files: manifest.files.len(),
            cases: manifest.cases.len(),
        }),
        Err(error) => Err(format!(
            "stat legacy source root {}: {error}",
            live_root.display()
        )),
    }
}

fn verify_live_source(
    ledger: &Value,
    manifest: &OracleManifest,
    live_root: &Path,
) -> Result<LegacyOracleReport, String> {
    let files = collect_regular_files(live_root)?;
    let actual: BTreeSet<&str> = files.iter().map(String::as_str).collect();

    let prefix = format!("{}/", manifest.source_root);
    let mut expected = BTreeSet::new();
    for file in &manifest.files {
        let relative = file.file.strip_prefix(&prefix).ok_or_else(|| {
            format!(
                "manifest file is outside {}: {}",
                manifest.source_root, file.file
            )
        })?;
        expected.insert(relative.to_owned());
    }
    let support_relative = manifest
        .support
        .path
        .strip_prefix(&prefix)
        .ok_or_else(|| format!("support.ts is outside {}", manifest.source_root))?
        .to_owned();
    expected.insert(support_relative);

    let expected_refs: BTreeSet<&str> = expected.iter().map(String::as_str).collect();
    if actual != expected_refs {
        let missing: Vec<&str> = expected_refs.difference(&actual).copied().collect();
        let extra: Vec<&str> = actual.difference(&expected_refs).copied().collect();
        return Err(format!(
            "live legacy source tree {} does not match the frozen inventory; missing: [{}], extra: [{}]",
            live_root.display(),
            missing.join(", "),
            extra.join(", ")
        ));
    }

    let mut sources = BTreeMap::new();
    for file in &manifest.files {
        let relative = file
            .file
            .strip_prefix(&prefix)
            .ok_or_else(|| format!("manifest file is outside {}", manifest.source_root))?;
        let path = live_root.join(relative);
        let bytes = fs::read(&path)
            .map_err(|error| format!("read live source {}: {error}", path.display()))?;
        let sha256 = hash_bytes(&bytes);
        if sha256 != file.sha256 {
            return Err(format!(
                "live file hash drift for {}: live {sha256} != manifest {}",
                file.file, file.sha256
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|error| format!("live source {} is not UTF-8: {error}", path.display()))?;
        let lines = line_count(&text)?;
        if lines != file.lines {
            return Err(format!(
                "live line-count drift for {}: live {lines} != manifest {}",
                file.file, file.lines
            ));
        }
        sources.insert(file.file.as_str(), text);
    }

    let support_path = live_root.join(
        manifest
            .support
            .path
            .strip_prefix(&prefix)
            .ok_or_else(|| format!("support.ts is outside {}", manifest.source_root))?,
    );
    let support_bytes = fs::read(&support_path)
        .map_err(|error| format!("read live support {}: {error}", support_path.display()))?;
    let support_sha = hash_bytes(&support_bytes);
    if support_sha != manifest.support.sha256 {
        return Err(format!(
            "live support.ts hash drift: live {support_sha} != manifest {}",
            manifest.support.sha256
        ));
    }
    let support_text = String::from_utf8(support_bytes)
        .map_err(|error| format!("live support.ts is not UTF-8: {error}"))?;
    if line_count(&support_text)? != manifest.support.lines {
        return Err("live support.ts line-count drift".to_owned());
    }

    let ledger_cases = required_array(
        ledger
            .as_object()
            .ok_or_else(|| "ledger root must be an object".to_owned())?,
        "cases",
        "ledger",
    )?;
    for case in &manifest.cases {
        let text = sources
            .get(case.file.as_str())
            .ok_or_else(|| format!("case {} references unread file {}", case.case_id, case.file))?;
        let line = usize::try_from(case.line)
            .map_err(|_| format!("case {} line does not fit usize", case.case_id))?;
        let source_line = text.lines().nth(line - 1).ok_or_else(|| {
            format!(
                "case {} points past the end of {}:{}",
                case.case_id, case.file, case.line
            )
        })?;
        if !source_scan::is_test_declaration(source_line) {
            return Err(format!(
                "case {} points at {}:{}, which is not a test declaration: {source_line:?}",
                case.case_id, case.file, case.line
            ));
        }
        let range = source_scan::test_call_range(text, line).ok_or_else(|| {
            format!(
                "case {} test call range was not found at {}:{}",
                case.case_id, case.file, case.line
            )
        })?;
        let body_sha256 = hash_bytes(text[range].as_bytes());
        if body_sha256 != case.body_sha256 {
            return Err(format!(
                "live body_sha256 drift for {} at {}:{}: live {body_sha256} != manifest {}",
                case.case_id, case.file, case.line, case.body_sha256
            ));
        }
    }
    if ledger_cases.len() != manifest.cases.len() {
        return Err("ledger.cases length drifted during live verification".to_owned());
    }

    Ok(LegacyOracleReport {
        mode: LegacySourceMode::Live,
        files: manifest.files.len(),
        cases: manifest.cases.len(),
    })
}

/// Collects regular files under `root` as sorted root-relative `/` paths.
///
/// Symlinks and special files are rejected, matching the frozen tree algorithm.
pub(crate) fn collect_regular_files(root: &Path) -> Result<Vec<String>, String> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("symlink_metadata {}: {error}", root.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing symlinked legacy source root: {}",
            root.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "legacy source root must be a directory: {}",
            root.display()
        ));
    }
    let mut files = Vec::new();
    walk_regular_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    Ok(files)
}

fn walk_regular_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<String>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("read_dir {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("read_dir entry under {}: {error}", directory.display()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("symlink_metadata {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing symlink in legacy source tree: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            walk_regular_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(slash_path(root, &path)?);
        } else {
            return Err(format!(
                "refusing non-regular file in legacy source tree: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn slash_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(root).map_err(|error| {
        format!(
            "{} is not under {}: {error}",
            path.display(),
            root.display()
        )
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| format!("non-UTF-8 path component in {}", path.display()))?,
            ),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(format!("path must be relative: {}", path.display()));
            }
        }
    }
    Ok(parts.join("/"))
}

fn line_count(text: &str) -> Result<u64, String> {
    u64::try_from(text.lines().count()).map_err(|_| "line count overflowed u64".to_owned())
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_case_id(value: &str) -> bool {
    value.starts_with("TS-")
        && value.len() > 4
        && value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn validate_relative(path: &str, context: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!(
            "{context} path must be a clean relative path: {path}"
        ));
    }
    Ok(())
}

fn require_strict_path_order<'a>(
    values: impl Iterator<Item = &'a str>,
    context: &str,
) -> Result<(), String> {
    let mut previous: Option<&str> = None;
    for value in values {
        if let Some(previous) = previous {
            if previous.as_bytes() >= value.as_bytes() {
                return Err(format!(
                    "{context} must be sorted by LC_ALL=C byte order: \
                     {previous:?} precedes {value:?}"
                ));
            }
        }
        previous = Some(value);
    }
    Ok(())
}

fn require_exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), String> {
    if object.len() != expected.len() {
        let actual: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = expected.iter().copied().collect();
        let missing: Vec<&str> = expected.difference(&actual).copied().collect();
        let extra: Vec<&str> = actual.difference(&expected).copied().collect();
        return Err(format!(
            "{context} has an unexpected key set; missing: [{}], extra: [{}]",
            missing.join(", "),
            extra.join(", ")
        ));
    }
    for key in expected {
        if !object.contains_key(*key) {
            return Err(format!("{context} is missing key {key}"));
        }
    }
    Ok(())
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a Map<String, Value>, String> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{context}.{key} must be a JSON object"))
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a [Value], String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context}.{key} must be an array"))
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context}.{key} must be a string"))
}

fn required_u64(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64, String> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context}.{key} must be an unsigned integer"))
}

#[cfg(test)]
#[path = "oracle_manifest_tests.rs"]
mod tests;
