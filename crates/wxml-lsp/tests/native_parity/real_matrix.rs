//! Real pinned-engine parity for the 33 in-place matrix scenarios.
//!
//! This module is the required-native half of the `scenario_matrix`
//! partition. It does **not** replay native output into expectations: the
//! frozen projection is derived from
//! `docs/evidence/data/type-bridge-c1/matrix.json`
//! (SHA-256 `69bc6c57405f664ad3195b566be7b821a5273fe807a8b58f1eb08b93b1afd6db`,
//! 577,049 bytes, TypeScript 6.0.3, in-place-shadow only). There is no
//! `BLESS`/`UPDATE_EXPECT`/`INSTA_UPDATE` path; the projection is read-only.
//!
//! Coverage is honest, not total:
//!
//! - 22 scenarios can be asserted strictly (20 checker scenarios plus
//!   the `interface-root`/`data-shorthand` short paths).
//! - 11 scenarios stay blocked by product/contract facts: 3 `documentationSource`
//!   (DOCS), 5 `Diagnostics=Disabled` (STATE), 1 probe health (PROBE), 1
//!   checker data-root classification (ROOT) and `page-closure` (CHECKER).
//!   Their checker half is still executed and characterized, but their product
//!   state is never fabricated.
//! - The 10 top-level cases keep their symbol-bridge half retired; only the
//!   in-place half is covered, and only `TS-MATRIX-0092/0228/0301/0319` has an
//!   in-place exact half.
//!
//! Default tests prove the projection's provenance and the comparator's
//! reverse failure. The `#[ignore]` tests are the required native job:
//! `WXML_TEST_NATIVE=1` must be set explicitly and `TSGO_BIN`/assets must be
//! pinned; without `WXML_REQUIRE_REAL_ENGINE=1` a missing engine is an explicit
//! printed skip, which is never ledger evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use wxml_backend_tsgo::config::{ENGINE_VERSION, ENGINE_VERSION_LINE, PinnedEngine};
use wxml_backend_tsgo::input::TypeAssets;
use wxml_backend_tsgo::input::{
    AnalysisInput, CompilerOptions, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision,
    SourceFile,
};
use wxml_backend_tsgo::native::session::{NativeSession, SessionOptions, TypeQuery};
use wxml_backend_tsgo::native::strings::{
    Utf16Offset, Utf16Position, offset_to_position, position_to_offset,
};
use wxml_backend_tsgo::native::wire::RawDiagnostic;
use wxml_backend_tsgo::query::definition::{DefinitionResult, DefinitionTarget, map_definition};
use wxml_backend_tsgo::query::diagnostics::{
    DiagnosticSeverity, NativeDiagnostic, map_diagnostics, map_diagnostics_for_file,
};
use wxml_backend_tsgo::query::hover::HoverEvidence;
use wxml_backend_tsgo::shadow::ShadowError;
use wxml_backend_tsgo::shadow::layout::{
    BuildStatus, BuiltShadow, DataRootKind, FragmentSpec, JsCheckApplied, JsCheckPolicy, RootKind,
    ShadowRequest, build_in_place_shadow,
};
use wxml_backend_tsgo::shadow::mapping::{
    MountRecord, SourceRange, map_span_to_source, utf16_to_byte,
};
use wxml_backend_tsgo::shadow::rewrite::Assembled;

use crate::ledger;
use crate::native_gate_env::{
    GateEnv, NATIVE_GATE_ENV, REQUIRE_REAL_ENGINE_ENV, TSGO_ASSET_ROOT_ENV, TSGO_BIN_ENV,
    TSGO_TYPINGS_ROOT_ENV,
};
use crate::native_package::TestRoot;

/// Frozen projection derived from the historical TypeScript 6.0.3 oracle.
pub(crate) const PROJECTION_RELATIVE_PATH: &str =
    "crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/projection.json";
/// Historical oracle artifact; never read at native runtime, only hashed.
pub(crate) const ORACLE_RELATIVE_PATH: &str = "docs/evidence/data/type-bridge-c1/matrix.json";
/// Fixture sample root copied byte-for-byte from `backends/typescript/samples`.
pub(crate) const SAMPLES_RELATIVE_PATH: &str =
    "crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/samples";
/// Exact historical oracle identity.
pub(crate) const ORACLE_SHA256: &str =
    "69bc6c57405f664ad3195b566be7b821a5273fe807a8b58f1eb08b93b1afd6db";
/// Exact historical oracle size.
pub(crate) const ORACLE_SIZE: u64 = 577_049;
/// Frozen scenario count.
pub(crate) const SCENARIOS: usize = 33;
/// Frozen exact in-place case count.
pub(crate) const EXACT_CASES: usize = 22;
/// Frozen blocked in-place case count.
pub(crate) const BLOCKED_CASES: usize = 11;
/// Required native job label used in diagnostics.
const JOB: &str = "real_matrix";
/// Marker printed by a required native run for CI audit.
const SUMMARY_PREFIX: &str = "real_matrix:";
/// Exact expected standard-library + typings file count.
const EXPECTED_ASSET_FILES: usize = 136;

/// The ignored required-native tests declared by this module.
///
/// `native_gates.rs` subtracts these from the ledger gate set; they are
/// matrix jobs, not P5/P6 gate replacements.
pub(crate) const IGNORED_MATRIX_TESTS: &[(&str, &str)] = &[
    (
        "crates/wxml-lsp/tests/native_parity/real_matrix.rs",
        "real_matrix_in_place_matches_frozen_oracle",
    ),
    (
        "crates/wxml-lsp/tests/native_parity/real_matrix.rs",
        "real_matrix_known_gaps_are_characterized",
    ),
    (
        "crates/wxml-lsp/tests/native_parity/real_matrix.rs",
        "real_matrix_top_level_cases",
    ),
];

/// One frozen projection file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    schema: String,
    source: ProjectionSource,
    #[serde(rename = "projectionSha256")]
    digest: String,
    scenarios: Vec<Scenario>,
    top_level_cases: Vec<TopLevelCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionSource {
    path: String,
    sha256: String,
    size: u64,
    environment: Value,
    selection: Selection,
    scenario_manifest: ScenarioSource,
    sample_files: BTreeMap<String, SampleFile>,
}

#[derive(Debug, Deserialize)]
struct Selection {
    layout: String,
    scenarios: usize,
    #[serde(rename = "inPlaceDiagnostics")]
    in_place_diagnostics: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioSource {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct SampleFile {
    sha256: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scenario {
    id: String,
    case_id: String,
    classification: Classification,
    sample_dir: String,
    script: String,
    extra_files: Vec<String>,
    registration: String,
    allow_js: bool,
    check_js: bool,
    js_check: String,
    compiler_options: BTreeMap<String, Value>,
    wxml: String,
    fragments: Vec<Fragment>,
    queries: Vec<Query>,
    expected_diagnostics: Vec<ExpectedDiagnostic>,
    oracle: Oracle,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Classification {
    status: String,
    blocked_by: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fragment {
    expr: String,
    target: Option<String>,
    wxml_range: FrozenRange,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
struct FrozenRange {
    start: u32,
    end: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Query {
    expr: String,
    target: Option<String>,
    expect_type: Option<String>,
    expect_doc: Option<String>,
    expect_doc_source: Option<String>,
    #[serde(default)]
    definition: bool,
    expect_definition: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedDiagnostic {
    expr: String,
    target: String,
    range: FrozenRange,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Oracle {
    states: BTreeMap<String, OracleState>,
    js_check_applied: String,
    module_before: String,
    module_after: String,
    diagnostics_exact: bool,
    perturbs_user_types: bool,
    user_script_diagnostics_introduced: Vec<u32>,
    registration: OracleRegistration,
    request: OracleRequest,
    mounts: Vec<OracleMount>,
    hovers: Vec<OracleHover>,
    definitions: Vec<OracleDefinition>,
    diagnostics: Vec<OracleDiagnostic>,
}

#[derive(Debug, Deserialize)]
struct OracleState {
    status: String,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OracleRegistration {
    kind: String,
    count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OracleRequest {
    owner_kind: String,
    fragment_count: usize,
    file_count: usize,
}

#[derive(Debug, Deserialize)]
struct OracleMount {
    #[serde(rename = "fragmentId")]
    fragment_id: String,
    target: String,
}

#[derive(Debug, Deserialize)]
struct OracleHover {
    query: String,
    observation: Option<OracleHoverObservation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OracleHoverObservation {
    type_only: Option<String>,
    documentation: Option<String>,
    documentation_source: Option<String>,
    quality: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OracleDefinition {
    query: String,
    observation: Option<OracleDefinitionObservation>,
}

#[derive(Debug, Deserialize)]
struct OracleDefinitionObservation {
    kind: String,
}

#[derive(Debug, Deserialize)]
struct OracleDiagnostic {
    code: u32,
    range: FrozenRange,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TopLevelCase {
    case_id: String,
    title: String,
    in_place_half: String,
    blocked_by: Option<String>,
    note: Option<String>,
}

/// The frozen `this.<member>` comparison result.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    offsets: Vec<u32>,
    types: Vec<String>,
}

/// Strict diagnostic comparison.
#[derive(Debug, Default)]
struct DiagnosticComparison {
    missing: Vec<(u32, u32, u32)>,
    unexpected: Vec<(u32, u32, u32)>,
    scenarios: usize,
}

impl DiagnosticComparison {
    fn exact(&self) -> bool {
        self.scenarios > 0 && self.missing.is_empty() && self.unexpected.is_empty()
    }
}

fn workspace() -> PathBuf {
    ledger::workspace_root()
}

fn load_projection_value() -> Value {
    let path = workspace().join(PROJECTION_RELATIVE_PATH);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn load_projection() -> Projection {
    serde_json::from_value(load_projection_value())
        .unwrap_or_else(|error| panic!("projection shape: {error}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(out, "{byte:02x}").expect("hex write");
    }
    out
}

/// Converts a byte offset/length in `text` into an absolute byte range.
fn byte_range(text: &str, start: usize, length: usize) -> FrozenRange {
    let slice = &text[start..start + length];
    FrozenRange {
        start: u32::try_from(start).expect("range start"),
        end: u32::try_from(start + slice.len()).expect("range end"),
    }
}

/// Locates the unique `{{ expr }}` occurrence and returns its byte range.
fn locate_expression(wxml: &str, expr: &str, scenario: &str) -> FrozenRange {
    let mut matches = Vec::new();
    let mut cursor = 0usize;
    while let Some(open) = wxml[cursor..].find("{{") {
        let open = cursor + open;
        let Some(close_rel) = wxml[open + 2..].find("}}") else {
            break;
        };
        let close = open + 2 + close_rel;
        let inner = &wxml[open + 2..close];
        if inner.trim() == expr {
            let leading = inner.len() - inner.trim_start().len();
            matches.push(open + 2 + leading);
        }
        cursor = open + 2;
    }
    assert_eq!(
        matches.len(),
        1,
        "scenario {scenario}: expression {expr:?} must be unique in WXML"
    );
    byte_range(wxml, matches[0], expr.len())
}

fn expected_diagnostic_tuples(scenario: &Scenario) -> BTreeSet<(u32, u32, u32)> {
    scenario
        .oracle
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                diagnostic.range.start,
                diagnostic.range.end,
            )
        })
        .collect()
}

fn compare_diagnostics(
    expected: &BTreeSet<(u32, u32, u32)>,
    actual: &BTreeSet<(u32, u32, u32)>,
    scenarios: usize,
) -> DiagnosticComparison {
    DiagnosticComparison {
        missing: expected.difference(actual).copied().collect(),
        unexpected: actual.difference(expected).copied().collect(),
        scenarios,
    }
}

fn root_kind(name: &str) -> RootKind {
    match name {
        "page" => RootKind::Page,
        "component" => RootKind::Component,
        "behavior" => RootKind::Behavior,
        other => panic!("unknown registration root: {other}"),
    }
}

fn js_check_policy(name: &str) -> JsCheckPolicy {
    match name {
        "inherit" => JsCheckPolicy::Inherit,
        "pragma" => JsCheckPolicy::Pragma,
        "off" => JsCheckPolicy::Off,
        other => panic!("unknown jsCheck policy: {other}"),
    }
}

const fn js_check_name(applied: JsCheckApplied) -> &'static str {
    match applied {
        JsCheckApplied::Inherit => "inherit",
        JsCheckApplied::Pragma => "pragma",
        JsCheckApplied::Off => "off",
    }
}

const fn build_status_name(status: BuildStatus) -> &'static str {
    match status {
        BuildStatus::Supported => "supported",
        BuildStatus::Partial => "partial",
    }
}

const fn registration_kind_name(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Page => "page",
        RootKind::Component => "component",
        RootKind::Behavior => "behavior",
    }
}

fn frozen_root(scenario: &Scenario) -> PathBuf {
    workspace()
        .join(SAMPLES_RELATIVE_PATH)
        .join(&scenario.sample_dir)
}

fn read_fixture(scenario: &Scenario, relative: &str) -> String {
    let path = frozen_root(scenario).join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn source_file(path: &str, text: &str) -> SourceFile {
    SourceFile::new(path, text, DocumentRevision::new(1))
        .unwrap_or_else(|error| panic!("source file {path}: {error}"))
}

fn project_options(scenario: &Scenario, project_root: &str) -> CompilerOptions {
    let mut values = BTreeMap::new();
    for (key, value) in &scenario.compiler_options {
        let value = match value {
            Value::String(text) if text == "<project-root>" => {
                Value::String(project_root.to_owned())
            }
            other => other.clone(),
        };
        values.insert(key.clone(), value);
    }
    CompilerOptions::new(values)
        .unwrap_or_else(|error| panic!("scenario {}: compiler options: {error}", scenario.id))
}

fn analysis_input(
    scenario: &Scenario,
    project_root: &str,
    revision: u64,
    files: Vec<SourceFile>,
    assets: &Arc<TypeAssets>,
) -> AnalysisInput {
    AnalysisInput::with_environment(
        ProjectKey::new("real-matrix"),
        ProjectRoot::new(project_root).expect("project root"),
        SnapshotRevision::new(revision),
        files,
        project_options(scenario, project_root),
        Arc::clone(assets),
    )
    .unwrap_or_else(|error| panic!("scenario {}: analysis input: {error}", scenario.id))
}

fn fragment_specs(scenario: &Scenario) -> Vec<FragmentSpec> {
    scenario
        .fragments
        .iter()
        .map(|fragment| {
            let mut spec = FragmentSpec::new(
                fragment.expr.clone(),
                fragment.expr.clone(),
                SourceRange::new(fragment.wxml_range.start, fragment.wxml_range.end),
            );
            spec.target.clone_from(&fragment.target);
            spec
        })
        .collect()
}

fn is_identifier_start(character: char) -> bool {
    character.is_alphabetic() || character == '_' || character == '$' || character == '\\'
}

fn is_identifier_continue(character: char) -> bool {
    character.is_alphanumeric() || is_identifier_start(character)
}

/// A lightweight lexer for `this.<identifier>` member offsets.
///
/// It skips comments, single/double quoted strings and template literals. The
/// frozen scenarios contain no regex literal and no `${}` substitution inside
/// a template, so the template body is skipped as text; this is a bounded
/// fixture scanner, not a general JavaScript parser.
fn this_member_offsets(text: &str) -> Vec<u32> {
    let mut offsets = Vec::new();
    let mut index = 0usize;
    let mut utf16 = 0u32;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut cursor = 0usize;
    while cursor < chars.len() {
        let (byte, character) = chars[cursor];
        let next = chars.get(cursor + 1).map(|(_, next)| *next);
        if character == '/' && next == Some('/') {
            while cursor < chars.len() && chars[cursor].1 != '\n' {
                utf16 += u32::try_from(chars[cursor].1.len_utf16()).unwrap_or(1);
                cursor += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            cursor += 2;
            utf16 += 2;
            while cursor < chars.len() {
                if chars[cursor].1 == '*' && chars.get(cursor + 1).map(|(_, c)| *c) == Some('/') {
                    cursor += 2;
                    utf16 += 2;
                    break;
                }
                utf16 += u32::try_from(chars[cursor].1.len_utf16()).unwrap_or(1);
                cursor += 1;
            }
            continue;
        }
        if character == '\'' || character == '"' || character == '`' {
            let quote = character;
            cursor += 1;
            utf16 += u32::try_from(quote.len_utf16()).unwrap_or(1);
            while cursor < chars.len() {
                let current = chars[cursor].1;
                if current == '\\' {
                    cursor += 1;
                    utf16 += 1;
                    if cursor < chars.len() {
                        utf16 += u32::try_from(chars[cursor].1.len_utf16()).unwrap_or(1);
                        cursor += 1;
                    }
                    continue;
                }
                utf16 += u32::try_from(current.len_utf16()).unwrap_or(1);
                cursor += 1;
                if current == quote {
                    break;
                }
            }
            continue;
        }
        if is_identifier_start(character) {
            let start = cursor;
            while cursor < chars.len() && is_identifier_continue(chars[cursor].1) {
                utf16 += u32::try_from(chars[cursor].1.len_utf16()).unwrap_or(1);
                cursor += 1;
            }
            if &text[byte..chars.get(cursor).map_or(text.len(), |(byte, _)| *byte)] != "this" {
                continue;
            }
            let mut probe = cursor;
            let mut probe_utf16 = utf16;
            while probe < chars.len() {
                let current = chars[probe].1;
                if current.is_whitespace() {
                    probe_utf16 += u32::try_from(current.len_utf16()).unwrap_or(1);
                    probe += 1;
                    continue;
                }
                break;
            }
            if probe < chars.len() && chars[probe].1 == '.' {
                probe += 1;
                probe_utf16 += 1;
                while probe < chars.len() && chars[probe].1.is_whitespace() {
                    probe_utf16 += u32::try_from(chars[probe].1.len_utf16()).unwrap_or(1);
                    probe += 1;
                }
                if probe < chars.len() && is_identifier_start(chars[probe].1) {
                    offsets.push(probe_utf16);
                }
            }
            let _ = start;
            continue;
        }
        utf16 += u32::try_from(character.len_utf16()).unwrap_or(1);
        cursor += 1;
        let _ = index;
        index += character.len_utf8();
    }
    offsets
}

fn position_of(text: &str, offset: u32) -> Utf16Position {
    offset_to_position(text, Utf16Offset::new(offset))
        .unwrap_or_else(|error| panic!("offset {offset} outside text: {error}"))
}

fn offset_of(text: &str, position: Utf16Position) -> u32 {
    position_to_offset(text, position)
        .unwrap_or_else(|error| panic!("position {position:?} outside text: {error}"))
        .get()
}

fn type_text(query: &TypeQuery) -> String {
    query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("<none>")
        .to_owned()
}

fn fingerprint_types(
    session: &mut NativeSession,
    path: &str,
    text: &str,
    offsets: &[u32],
) -> Vec<String> {
    offsets
        .iter()
        .map(|offset| {
            let query = session
                .type_query(path, position_of(text, *offset))
                .unwrap_or_else(|error| panic!("type query at {offset}: {error}"));
            type_text(&query)
        })
        .collect()
}

fn fingerprint_is_meaningful(fingerprint: &Fingerprint) -> bool {
    !fingerprint.offsets.is_empty() && fingerprint.types.iter().all(|text| text != "<none>")
}

fn fingerprint_perturbs(baseline: &Fingerprint, shadow: &Fingerprint) -> bool {
    baseline.types != shadow.types
}

#[derive(Debug, Clone)]
struct MappedDiagnostic {
    range: Option<SourceRange>,
    suppressed: bool,
    unmapped: bool,
}

/// Maps one shadow diagnostic back to WXML bytes.
fn map_shadow_diagnostic(
    diagnostic: &NativeDiagnostic,
    shadow_text: &str,
    mounts: &[MountRecord],
) -> MappedDiagnostic {
    let start = offset_of(shadow_text, diagnostic.start);
    let end = offset_of(shadow_text, diagnostic.end);
    let Some(mount) = mounts
        .iter()
        .find(|mount| mount.start_utf16 <= start && start < mount.end_utf16().unwrap_or(u32::MAX))
    else {
        return MappedDiagnostic {
            range: None,
            suppressed: true,
            unmapped: false,
        };
    };
    let length = end.saturating_sub(start);
    let Ok(mapped) = map_span_to_source(mount, Utf16Offset::new(start), length) else {
        return MappedDiagnostic {
            range: None,
            suppressed: false,
            unmapped: true,
        };
    };
    MappedDiagnostic {
        range: mapped.range,
        suppressed: false,
        unmapped: mapped.unattributable || mapped.range.is_none(),
    }
}

fn baseline_diagnostic_keys(semantic: &[RawDiagnostic], text: &str) -> Vec<(u32, u32, u32)> {
    let mapped = map_diagnostics(semantic, &[], text);
    assert!(
        matches!(mapped.status, wxml_types::CapabilityStatus::Supported),
        "baseline diagnostics were dropped by range validation: {:?}",
        mapped.status
    );
    mapped
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let start = offset_of(text, diagnostic.start);
            let end = offset_of(text, diagnostic.end);
            (diagnostic.code, start, end.saturating_sub(start))
        })
        .collect()
}

/// Port of the oracle's `introducedDiagnostics` keying rule.
fn introduced_codes(
    baseline: &[(u32, u32, u32)],
    shadow: &[NativeDiagnostic],
    shadow_text: &str,
    mounts: &[MountRecord],
    map: &wxml_backend_tsgo::shadow::mapping::ShadowMap,
) -> Vec<u32> {
    let mut remaining: BTreeMap<(u32, u32, u32), usize> = BTreeMap::new();
    for key in baseline {
        *remaining.entry(*key).or_default() += 1;
    }
    let mut introduced = Vec::new();
    for diagnostic in shadow {
        let start = offset_of(shadow_text, diagnostic.start);
        let end = offset_of(shadow_text, diagnostic.end);
        let length = end.saturating_sub(start);
        if mounts.iter().any(|mount| {
            mount.start_utf16 <= start && start < mount.end_utf16().unwrap_or(u32::MAX)
        }) {
            continue;
        }
        let key = map
            .to_original_offset(Utf16Offset::new(start))
            .map_or((diagnostic.code, u32::MAX, length), |original| {
                (diagnostic.code, original.get(), length)
            });
        if let Some(available) = remaining.get_mut(&key)
            && *available > 0
        {
            *available -= 1;
            continue;
        }
        introduced.push(diagnostic.code);
    }
    introduced.truncate(8);
    introduced
}

#[derive(Debug, Clone)]
enum BuiltFacts {
    Unsupported {
        feature: &'static str,
        reason: String,
    },
    Built {
        status: BuildStatus,
        reason: Option<String>,
        js_check: JsCheckApplied,
        registration_kind: RootKind,
        registration_count: usize,
        data_root: DataRootKind,
        mounts: Vec<MountFact>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountFact {
    fragment_id: String,
}

#[derive(Debug)]
struct ScenarioObservation {
    built: BuiltFacts,
    derived_states: BTreeMap<&'static str, String>,
    diagnostics: BTreeSet<(u32, u32, u32)>,
    introduced: Vec<u32>,
    fingerprint: Fingerprint,
    fingerprint_baseline: Fingerprint,
    hovers: BTreeMap<String, HoverFact>,
    definitions: BTreeMap<String, DefinitionFact>,
    baseline_diagnostics: usize,
    suppressed: usize,
    unmapped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HoverFact {
    type_text: String,
    documentation: Option<String>,
    symbol: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefinitionFact {
    kind: String,
    range_non_empty: bool,
    generated_source_non_empty: bool,
}

fn build_facts(build: &Result<BuiltShadow, ShadowError>) -> BuiltFacts {
    match build {
        Err(ShadowError::Unsupported { feature, reason }) => BuiltFacts::Unsupported {
            feature,
            reason: reason.clone(),
        },
        Err(error) => panic!("unexpected shadow error: {error}"),
        Ok(shadow) => BuiltFacts::Built {
            status: shadow.status,
            reason: shadow.reason.clone(),
            js_check: shadow.js_check_applied,
            registration_kind: shadow.registration.kind,
            registration_count: shadow.registration.count,
            data_root: shadow.data_root,
            mounts: shadow
                .view
                .mounts
                .iter()
                .map(|mount| MountFact {
                    fragment_id: mount.fragment_id.clone(),
                })
                .collect(),
        },
    }
}

fn shadow_build(scenario: &Scenario) -> Result<BuiltShadow, ShadowError> {
    let script = read_fixture(scenario, &scenario.script);
    let script = source_file(&scenario.script, &script);
    let fragments = fragment_specs(scenario);
    let request = ShadowRequest {
        script: &script,
        root: root_kind(&scenario.registration),
        fragments: &fragments,
        js_check: js_check_policy(&scenario.js_check),
        check_js: scenario.check_js,
    };
    build_in_place_shadow(&request)
}

fn assert_unsupported(scenario: &Scenario, facts: &BuiltFacts) {
    let BuiltFacts::Unsupported { feature, reason } = facts else {
        panic!(
            "scenario {}: expected Unsupported build, got {facts:?}",
            scenario.id
        );
    };
    let marker = match scenario.id.as_str() {
        "interface-root" => "type-only",
        "data-shorthand" => "not a property assignment",
        other => panic!("scenario {other}: unexpected Unsupported short path"),
    };
    assert!(
        reason.contains(marker),
        "scenario {}: Unsupported reason {reason:?} must contain {marker:?}",
        scenario.id
    );
    assert_eq!(
        *feature, "data.root",
        "scenario {}: short path must be a data-root rejection",
        scenario.id
    );
}

fn assert_partial_reason(scenario: &Scenario, facts: &BuiltFacts) {
    let BuiltFacts::Built { status, reason, .. } = facts else {
        panic!(
            "scenario {}: expected a partial build, got {facts:?}",
            scenario.id
        );
    };
    assert_eq!(
        *status,
        BuildStatus::Partial,
        "scenario {}: expected Partial build",
        scenario.id
    );
    let reason = reason
        .as_deref()
        .unwrap_or_else(|| panic!("scenario {}: Partial without reason", scenario.id));
    let marker = match scenario.id.as_str() {
        "properties-spread" => "spread assignment",
        "multi-registration" => "2 page registrations",
        "methods-nonliteral" => "object literal",
        "interface-root-import" => "data",
        "page-closure" => "inline object literal",
        other => panic!("scenario {other}: unexpected partial scenario"),
    };
    assert!(
        reason.contains(marker),
        "scenario {}: Partial reason {reason:?} must contain {marker:?}",
        scenario.id
    );
}

fn assert_supported_build(scenario: &Scenario, facts: &BuiltFacts) {
    match facts {
        BuiltFacts::Built { status, .. } => assert_eq!(
            *status,
            BuildStatus::Supported,
            "scenario {}: expected Supported build, got {facts:?}",
            scenario.id
        ),
        BuiltFacts::Unsupported { .. } => panic!(
            "scenario {}: expected Supported build, got Unsupported",
            scenario.id
        ),
    }
}

fn assert_build_facts(scenario: &Scenario, facts: &BuiltFacts) {
    if scenario.classification.status == "exact" {
        match scenario.id.as_str() {
            "interface-root" | "data-shorthand" => {
                assert_unsupported(scenario, facts);
                return;
            }
            "properties-spread" | "multi-registration" | "methods-nonliteral" => {
                assert_partial_reason(scenario, facts);
                if scenario.id == "methods-nonliteral" {
                    let BuiltFacts::Built { mounts, .. } = facts else {
                        unreachable!()
                    };
                    assert!(
                        mounts.is_empty(),
                        "scenario {}: no fragment can be mounted without a probe",
                        scenario.id
                    );
                }
            }
            _ => assert_supported_build(scenario, facts),
        }
    } else {
        match scenario.classification.blocked_by.as_deref() {
            Some("docs" | "probe") => assert_supported_build(scenario, facts),
            Some("state") => {
                assert_supported_build(scenario, facts);
                let BuiltFacts::Built { js_check, .. } = facts else {
                    unreachable!()
                };
                assert_eq!(
                    *js_check,
                    JsCheckApplied::Off,
                    "scenario {}: a STATE case must disable template checking",
                    scenario.id
                );
            }
            Some("checker") if scenario.id == "page-closure" => {
                assert_partial_reason(scenario, facts);
                let BuiltFacts::Built { data_root, .. } = facts else {
                    unreachable!()
                };
                assert_eq!(
                    *data_root,
                    DataRootKind::Expression,
                    "scenario {}: a generic-call data root stays Expression",
                    scenario.id
                );
            }
            Some("checker") => panic!("scenario {}: unknown checker gap", scenario.id),
            Some("root") => {
                assert_partial_reason(scenario, facts);
                let BuiltFacts::Built { data_root, .. } = facts else {
                    unreachable!()
                };
                assert_eq!(
                    *data_root,
                    DataRootKind::Expression,
                    "scenario {}: imported data root must stay Expression until ROOT classification",
                    scenario.id
                );
            }
            other => panic!("scenario {}: unknown blockedBy {other:?}", scenario.id),
        }
    }

    let BuiltFacts::Built {
        js_check,
        registration_kind,
        registration_count,
        mounts,
        ..
    } = facts
    else {
        return;
    };
    assert_eq!(
        js_check_name(*js_check),
        scenario.oracle.js_check_applied,
        "scenario {}: jsCheckApplied",
        scenario.id
    );
    assert_eq!(
        registration_kind_name(*registration_kind),
        scenario.oracle.registration.kind,
        "scenario {}: registration kind",
        scenario.id
    );
    assert_eq!(
        *registration_count, scenario.oracle.registration.count,
        "scenario {}: registration count",
        scenario.id
    );
    let expected_mounts: Vec<&str> = scenario
        .oracle
        .mounts
        .iter()
        .map(|mount| mount.fragment_id.as_str())
        .collect();
    let observed_mounts: Vec<&str> = mounts
        .iter()
        .map(|mount| mount.fragment_id.as_str())
        .collect();
    assert_eq!(
        observed_mounts, expected_mounts,
        "scenario {}: mounted fragment ids",
        scenario.id
    );
}

/// Derives the checker-half capability states from native build facts.
///
/// This is not a product DTO: the exact cases are exactly those whose states
/// can be derived from `status`/`js_check`/`introduced`/`unmapped` alone.
fn derive_states(
    facts: &BuiltFacts,
    introduced: &[u32],
    unmapped: usize,
    dropped_definitions: usize,
) -> BTreeMap<&'static str, String> {
    let mut states = BTreeMap::new();
    match facts {
        BuiltFacts::Unsupported { .. } => {
            for capability in ["hover", "diagnostics", "definition"] {
                states.insert(capability, "unsupported".to_owned());
            }
        }
        BuiltFacts::Built {
            status, js_check, ..
        } => {
            let partial = *status == BuildStatus::Partial;
            let hover = if partial { "partial" } else { "supported" };
            let mut diagnostics = if partial { "partial" } else { "supported" };
            let mut definition = if partial { "partial" } else { "supported" };
            if *js_check == JsCheckApplied::Off {
                diagnostics = "disabled";
            } else if !introduced.is_empty() || unmapped > 0 {
                diagnostics = "partial";
            }
            if dropped_definitions > 0 {
                definition = "partial";
            }
            states.insert("hover", hover.to_owned());
            states.insert("diagnostics", diagnostics.to_owned());
            states.insert("definition", definition.to_owned());
        }
    }
    states
}

fn classify_target(target: &DefinitionTarget, built: &BuiltShadow, script: &str) -> (String, bool) {
    if target.path == script {
        if built.view.generated_declarations.iter().any(|declaration| {
            let end = declaration
                .start_utf16
                .saturating_add(declaration.length_utf16);
            declaration.start_utf16 <= target.utf16.start.get() && target.utf16.start.get() < end
        }) {
            let source_non_empty = built
                .view
                .generated_declarations
                .iter()
                .any(|declaration| declaration.source.is_some_and(|range| !range.is_empty()));
            return ("generated".to_owned(), source_non_empty);
        }
        let mapped = built
            .view
            .map
            .to_original_offset(Utf16Offset::new(target.utf16.start.get()))
            .is_ok();
        return ("script".to_owned(), mapped && !target.range.is_empty());
    }
    ("external".to_owned(), !target.range.is_empty())
}

fn map_hover(hover: &HoverEvidence) -> HoverFact {
    HoverFact {
        type_text: hover
            .type_info
            .as_ref()
            .map(|info| info.text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "<none>".to_owned()),
        documentation: hover
            .documentation
            .as_ref()
            .and_then(|documentation| documentation.comment.clone())
            .map(|comment| comment.trim().to_owned())
            .filter(|comment| !comment.is_empty()),
        symbol: hover.symbol.is_some(),
    }
}

fn observe_scenario(
    scenario: &Scenario,
    engine: &EnginePaths,
    assets: &Arc<TypeAssets>,
    work: &TestRoot,
    index: usize,
) -> ScenarioObservation {
    let build = shadow_build(scenario);
    let facts = build_facts(&build);
    assert_build_facts(scenario, &facts);

    if let BuiltFacts::Unsupported { .. } = facts {
        return ScenarioObservation {
            derived_states: derive_states(&facts, &[], 0, 0),
            built: facts,
            diagnostics: BTreeSet::new(),
            introduced: Vec::new(),
            fingerprint: Fingerprint {
                offsets: Vec::new(),
                types: Vec::new(),
            },
            fingerprint_baseline: Fingerprint {
                offsets: Vec::new(),
                types: Vec::new(),
            },
            hovers: BTreeMap::new(),
            definitions: BTreeMap::new(),
            baseline_diagnostics: 0,
            suppressed: 0,
            unmapped: 0,
        };
    }
    let shadow_view = build.expect("built scenario");
    if scenario.id == "methods-nonliteral" {
        assert_eq!(
            shadow_view.view.text, shadow_view.view.original,
            "scenario {}: the partial build must keep the original script",
            scenario.id
        );
        assert!(
            shadow_view.view.edits.is_empty(),
            "scenario {}: the partial build must not edit the script",
            scenario.id
        );
        assert!(
            shadow_view.view.insertions.is_empty(),
            "scenario {}: the partial build must not inject a probe",
            scenario.id
        );
        assert!(
            shadow_view.view.mounts.is_empty(),
            "scenario {}: the partial build must not mount fragments",
            scenario.id
        );
    }
    for (mount, expected) in shadow_view.view.mounts.iter().zip(&scenario.oracle.mounts) {
        let start = utf16_to_byte(&shadow_view.view.text, mount.target_start_utf16).unwrap_or_else(
            |error| panic!("scenario {}: mount target start: {error}", scenario.id),
        );
        let end = utf16_to_byte(
            &shadow_view.view.text,
            mount.target_start_utf16 + mount.target_length_utf16,
        )
        .unwrap_or_else(|error| panic!("scenario {}: mount target end: {error}", scenario.id));
        assert_eq!(
            &shadow_view.view.text[start..end],
            expected.target,
            "scenario {}: mount target text",
            scenario.id
        );
    }

    let script_path = &scenario.script;
    let original = read_fixture(scenario, script_path);
    let sandbox = work.path().join(format!("matrix-{index:02}"));
    std::fs::create_dir(&sandbox)
        .unwrap_or_else(|error| panic!("create sandbox {}: {error}", sandbox.display()));
    let project_root = sandbox.join("project");
    assert!(
        !project_root.exists(),
        "the virtual project root must not exist on disk: {}",
        project_root.display()
    );
    let project_root_text = project_root
        .to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 project root {}", project_root.display()))
        .to_owned();

    let mut session = NativeSession::start(SessionOptions::new(&engine.executable, &sandbox))
        .unwrap_or_else(|error| panic!("scenario {}: start engine: {error}", scenario.id));
    let initialize = session
        .initialize_result()
        .expect("initialize result")
        .clone();
    assert_eq!(
        initialize.current_directory(),
        sandbox
            .to_str()
            .unwrap_or_else(|| panic!("non-UTF-8 sandbox {}", sandbox.display())),
        "scenario {}: engine currentDirectory",
        scenario.id
    );

    let mut baseline_files = vec![source_file(script_path, &original)];
    for extra in &scenario.extra_files {
        baseline_files.push(source_file(extra, &read_fixture(scenario, extra)));
    }
    let baseline = analysis_input(scenario, &project_root_text, 1, baseline_files, assets);
    session
        .open_project(&baseline)
        .unwrap_or_else(|error| panic!("scenario {}: open project: {error}", scenario.id));
    assert_eq!(session.live_snapshot_count(), 1);

    let offsets = this_member_offsets(&original);
    let baseline_types = fingerprint_types(&mut session, script_path, &original, &offsets);
    let baseline_raw = session
        .semantic_diagnostics(script_path)
        .unwrap_or_else(|error| panic!("scenario {}: baseline diagnostics: {error}", scenario.id));
    let baseline_keys = baseline_diagnostic_keys(&baseline_raw, &original);

    let shadow_script = shadow_view.view.text.clone();
    let mut shadow_files = vec![source_file(script_path, &shadow_script)];
    for extra in &scenario.extra_files {
        shadow_files.push(source_file(extra, &read_fixture(scenario, extra)));
    }
    let shadow = analysis_input(scenario, &project_root_text, 2, shadow_files, assets);
    session
        .update_project(&shadow)
        .unwrap_or_else(|error| panic!("scenario {}: update project: {error}", scenario.id));
    assert_eq!(session.live_snapshot_count(), 1);

    let shadow_offsets: Vec<u32> = offsets
        .iter()
        .map(|offset| {
            shadow_view
                .view
                .map
                .to_derived_offset(Utf16Offset::new(*offset))
                .unwrap_or_else(|error| {
                    panic!(
                        "scenario {}: map this offset {offset}: {error}",
                        scenario.id
                    )
                })
                .get()
        })
        .collect();
    let shadow_types =
        fingerprint_types(&mut session, script_path, &shadow_script, &shadow_offsets);
    assert_eq!(
        baseline_types.len(),
        shadow_types.len(),
        "scenario {}: fingerprint length",
        scenario.id
    );
    let fingerprint_baseline = Fingerprint {
        offsets,
        types: baseline_types,
    };
    let fingerprint = Fingerprint {
        offsets: shadow_offsets,
        types: shadow_types,
    };

    let evidence = session
        .diagnostics(script_path)
        .unwrap_or_else(|error| panic!("scenario {}: diagnostics: {error}", scenario.id));
    assert!(
        evidence.syntactic.is_empty(),
        "scenario {}: the shadow generated syntactic diagnostics: {:?}",
        scenario.id,
        evidence.syntactic
    );
    let mapped = map_diagnostics_for_file(
        &evidence.semantic,
        &evidence.syntactic,
        &shadow_script,
        &project_root_text,
        script_path,
    );
    assert_eq!(
        mapped.status,
        wxml_types::CapabilityStatus::Supported,
        "scenario {}: native diagnostics were dropped or foreign-owned",
        scenario.id
    );
    let mut diagnostics = BTreeSet::new();
    let mut suppressed = 0usize;
    let mut unmapped = 0usize;
    for diagnostic in &mapped.diagnostics {
        assert_eq!(
            diagnostic.severity,
            DiagnosticSeverity::Error,
            "scenario {}: matrix diagnostics must be errors",
            scenario.id
        );
        let mapped_diagnostic =
            map_shadow_diagnostic(diagnostic, &shadow_script, &shadow_view.view.mounts);
        if mapped_diagnostic.suppressed {
            suppressed += 1;
        } else if let Some(range) = mapped_diagnostic.range {
            if mapped_diagnostic.unmapped {
                unmapped += 1;
            } else {
                diagnostics.insert((diagnostic.code, range.start.get(), range.end.get()));
            }
        } else {
            unmapped += 1;
        }
    }
    assert_eq!(
        unmapped, 0,
        "scenario {}: in-mount diagnostics must map to WXML bytes",
        scenario.id
    );

    let introduced = introduced_codes(
        &baseline_keys,
        &mapped.diagnostics,
        &shadow_script,
        &shadow_view.view.mounts,
        &shadow_view.view.map,
    );

    let mut hovers = BTreeMap::new();
    let query_hovers = state_status(scenario, "hover") == "supported"
        || scenario.classification.blocked_by.as_deref() == Some("probe")
        || scenario.id == "multi-registration";
    if query_hovers {
        for query in &scenario.queries {
            let mount = shadow_view
                .view
                .mounts
                .iter()
                .find(|mount| mount.fragment_id == query.expr)
                .unwrap_or_else(|| {
                    panic!(
                        "scenario {}: no mount for hover query {}",
                        scenario.id, query.expr
                    )
                });
            let position = position_of(&shadow_script, mount.target_start_utf16);
            let hover = session
                .hover_evidence(script_path, position)
                .unwrap_or_else(|error| {
                    panic!(
                        "scenario {}: hover for {}: {error}",
                        scenario.id, query.expr
                    )
                });
            hovers.insert(query.expr.clone(), map_hover(&hover));
        }
    }

    let mut definitions = BTreeMap::new();
    let mut dropped_definitions = 0usize;
    if state_status(scenario, "definition") == "supported" {
        for query in &scenario.queries {
            if !query.definition {
                continue;
            }
            let mount = shadow_view
                .view
                .mounts
                .iter()
                .find(|mount| mount.fragment_id == query.expr)
                .unwrap_or_else(|| {
                    panic!(
                        "scenario {}: no mount for definition query {}",
                        scenario.id, query.expr
                    )
                });
            let position = position_of(&shadow_script, mount.target_start_utf16);
            let raw = session
                .definition(script_path, position)
                .unwrap_or_else(|error| {
                    panic!(
                        "scenario {}: definition for {}: {error}",
                        scenario.id, query.expr
                    )
                });
            let result = map_definition(&raw);
            let fact = definition_fact(scenario, &result, &shadow_view, script_path);
            if fact.kind == "dropped" {
                dropped_definitions += 1;
            } else {
                definitions.insert(query.expr.clone(), fact);
            }
        }
    }

    session.release().expect("release snapshot");
    assert_eq!(
        session.live_snapshot_count(),
        0,
        "scenario {}: live snapshots after release",
        scenario.id
    );
    session
        .shutdown()
        .unwrap_or_else(|| panic!("scenario {}: shutdown", scenario.id));
    assert!(
        !project_root.exists(),
        "scenario {}: virtual project root was written to disk",
        scenario.id
    );

    ScenarioObservation {
        derived_states: derive_states(&facts, &introduced, unmapped, dropped_definitions),
        built: facts,
        diagnostics,
        introduced,
        fingerprint,
        fingerprint_baseline,
        hovers,
        definitions,
        baseline_diagnostics: baseline_keys.len(),
        suppressed,
        unmapped,
    }
}

fn definition_fact(
    scenario: &Scenario,
    result: &DefinitionResult,
    built: &BuiltShadow,
    script: &str,
) -> DefinitionFact {
    assert!(
        !result.targets.is_empty(),
        "scenario {}: definition query must produce at least one target",
        scenario.id
    );
    let first = &result.targets[0];
    let (kind, range_non_empty) = classify_target(first, built, script);
    let generated_source_non_empty = kind != "generated"
        || built
            .view
            .generated_declarations
            .iter()
            .any(|declaration| declaration.source.is_some_and(|range| !range.is_empty()));
    DefinitionFact {
        kind,
        range_non_empty,
        generated_source_non_empty,
    }
}

fn state_status<'a>(scenario: &'a Scenario, capability: &str) -> &'a str {
    scenario
        .oracle
        .states
        .get(capability)
        .unwrap_or_else(|| panic!("scenario {}: missing {capability} state", scenario.id))
        .status
        .as_str()
}

fn assert_states(scenario: &Scenario, observation: &ScenarioObservation) {
    if scenario.classification.status != "exact" {
        return;
    }
    for capability in ["hover", "diagnostics", "definition"] {
        assert_eq!(
            observation
                .derived_states
                .get(capability)
                .map(String::as_str),
            Some(state_status(scenario, capability)),
            "scenario {}: derived {capability} state",
            scenario.id
        );
    }
}

fn assert_diagnostics(scenario: &Scenario, observation: &ScenarioObservation) {
    let expected = expected_diagnostic_tuples(scenario);
    let comparison = compare_diagnostics(&expected, &observation.diagnostics, 1);
    assert!(
        comparison.exact(),
        "scenario {}: diagnostics missing={:?} unexpected={:?}",
        scenario.id,
        comparison.missing,
        comparison.unexpected
    );
    assert!(observation.unmapped == 0, "scenario {}", scenario.id);
}

fn assert_hover_facts(scenario: &Scenario, observation: &ScenarioObservation) {
    if state_status(scenario, "hover") != "supported" {
        if scenario.classification.blocked_by.as_deref() == Some("probe") {
            assert!(
                observation.hovers.contains_key("title"),
                "scenario {}: the probe checker half must query title",
                scenario.id
            );
        } else if scenario.id == "multi-registration" {
            let title = observation
                .hovers
                .get("title")
                .unwrap_or_else(|| panic!("scenario {}: top-level title hover", scenario.id));
            assert_eq!(
                title.type_text, "string",
                "scenario {}: the first registration wins",
                scenario.id
            );
        } else {
            assert!(
                observation.hovers.is_empty(),
                "scenario {}: hover queries must be skipped when hover is {}",
                scenario.id,
                state_status(scenario, "hover")
            );
        }
        return;
    }
    for query in &scenario.queries {
        let oracle = scenario
            .oracle
            .hovers
            .iter()
            .find(|hover| hover.query == query.expr)
            .unwrap_or_else(|| panic!("scenario {}: oracle hover {}", scenario.id, query.expr));
        let observed = observation
            .hovers
            .get(&query.expr)
            .unwrap_or_else(|| panic!("scenario {}: observed hover {}", scenario.id, query.expr));
        if let Some(expect_type) = &query.expect_type {
            assert_eq!(
                &observed.type_text, expect_type,
                "scenario {}: hover type of {}",
                scenario.id, query.expr
            );
        } else if let Some(expected) = oracle.observation.as_ref()
            && let Some(expected) = &expected.type_only
            && expected != "any"
        {
            assert_eq!(
                &observed.type_text, expected,
                "scenario {}: oracle hover type of {}",
                scenario.id, query.expr
            );
        }
        if let Some(expect_doc) = &query.expect_doc {
            assert_eq!(
                observed.documentation.as_deref(),
                Some(expect_doc.as_str()),
                "scenario {}: hover documentation of {}",
                scenario.id,
                query.expr
            );
        }
        assert!(
            observed.symbol || observed.type_text != "<none>",
            "scenario {}: hover of {} has neither a type nor a symbol observation",
            scenario.id,
            query.expr
        );
    }
}

fn assert_definition_facts(scenario: &Scenario, observation: &ScenarioObservation) {
    if state_status(scenario, "definition") != "supported" {
        return;
    }
    for query in &scenario.queries {
        if !query.definition {
            continue;
        }
        let expected_kind = query
            .expect_definition
            .as_deref()
            .unwrap_or_else(|| panic!("scenario {}: definition without kind", scenario.id));
        let observed = observation.definitions.get(&query.expr).unwrap_or_else(|| {
            panic!(
                "scenario {}: missing definition for {}",
                scenario.id, query.expr
            )
        });
        assert_eq!(
            observed.kind, expected_kind,
            "scenario {}: definition kind of {}",
            scenario.id, query.expr
        );
        assert!(
            observed.range_non_empty,
            "scenario {}: definition range of {} must be non-empty",
            scenario.id, query.expr
        );
        if expected_kind == "generated" {
            assert!(
                observed.generated_source_non_empty,
                "scenario {}: generated definition of {} must map to source",
                scenario.id, query.expr
            );
        }
    }
}

fn assert_non_interference(scenario: &Scenario, observation: &ScenarioObservation) {
    if observation.fingerprint_baseline.offsets.is_empty() {
        assert!(
            !fingerprint_is_meaningful(&observation.fingerprint_baseline),
            "scenario {}: an empty fingerprint must not be meaningful",
            scenario.id
        );
    } else {
        assert!(
            fingerprint_is_meaningful(&observation.fingerprint_baseline),
            "scenario {}: this.<member> baselines must all resolve: {:?}",
            scenario.id,
            observation.fingerprint_baseline.types
        );
    }
    assert_eq!(
        fingerprint_perturbs(&observation.fingerprint_baseline, &observation.fingerprint),
        scenario.oracle.perturbs_user_types,
        "scenario {}: user type perturbation",
        scenario.id
    );
    assert_eq!(
        observation.introduced, scenario.oracle.user_script_diagnostics_introduced,
        "scenario {}: introduced script diagnostics",
        scenario.id
    );
}

fn assert_query_expectations(scenario: &Scenario) {
    for query in &scenario.queries {
        if query.expect_doc_source.is_some() {
            assert!(
                scenario.classification.blocked_by.as_deref() == Some("docs"),
                "scenario {}: documentationSource is only expected on a DOCS-blocked case",
                scenario.id
            );
        }
    }
}

fn assert_gap_expectation(scenario: &Scenario, observation: &ScenarioObservation) {
    match scenario.classification.blocked_by.as_deref() {
        Some("docs") => {
            assert!(
                scenario
                    .queries
                    .iter()
                    .any(|query| query.expect_doc_source.is_some()),
                "scenario {}: a DOCS gap must have a documentationSource expectation",
                scenario.id
            );
        }
        Some("state") => {
            assert_eq!(
                observation
                    .derived_states
                    .get("diagnostics")
                    .map(String::as_str),
                Some("disabled"),
                "scenario {}: the checker half must observe disabled template checking",
                scenario.id
            );
            assert!(
                observations_diagnostics(observation).is_empty(),
                "scenario {}: a STATE gap must publish no diagnostics",
                scenario.id
            );
        }
        Some("probe") => {
            assert!(
                observations_diagnostics(observation).is_empty(),
                "scenario {}: a PROBE gap must publish no diagnostics",
                scenario.id
            );
            let title = observation
                .hovers
                .get("title")
                .unwrap_or_else(|| panic!("scenario {}: probe hover title", scenario.id));
            assert_eq!(
                title.type_text, "any",
                "scenario {}: noImplicitThis:false makes the probe root any",
                scenario.id
            );
        }
        Some("checker") => {
            assert!(
                observations_diagnostics(observation).is_empty(),
                "scenario {}: a CHECKER gap must publish no diagnostics",
                scenario.id
            );
            match scenario.id.as_str() {
                "page-closure" => assert!(
                    matches!(
                        &observation.built,
                        BuiltFacts::Built {
                            status: BuildStatus::Partial,
                            ..
                        }
                    ),
                    "scenario {}: the current native build must stay Partial",
                    scenario.id
                ),
                other => panic!("scenario {other}: unknown checker gap"),
            }
        }
        Some("root") => {
            assert!(
                observations_diagnostics(observation).is_empty(),
                "scenario {}: a ROOT gap must publish no diagnostics",
                scenario.id
            );
            assert!(
                observation.hovers.is_empty(),
                "scenario {}: ROOT query values are not claimed",
                scenario.id
            );
        }
        other => panic!("scenario {}: unknown gap {other:?}", scenario.id),
    }
}

fn observations_diagnostics(observation: &ScenarioObservation) -> BTreeSet<(u32, u32, u32)> {
    observation.diagnostics.clone()
}

#[derive(Debug, Clone)]
struct EnginePaths {
    executable: PathBuf,
    asset_root: PathBuf,
    typings_root: PathBuf,
}

fn resolve_standard_lib_root(asset_root: &Path) -> Option<PathBuf> {
    [asset_root.to_path_buf(), asset_root.join("engine/lib")]
        .into_iter()
        .find(|candidate| candidate.join("lib.d.ts").is_file())
}

fn resolve_typings_root(root: &Path) -> Option<PathBuf> {
    if root
        .join("miniprogram-api-typings")
        .join("index.d.ts")
        .is_file()
    {
        return Some(root.to_path_buf());
    }
    let parent = root.parent()?;
    if parent
        .join("miniprogram-api-typings")
        .join("index.d.ts")
        .is_file()
    {
        return Some(parent.to_path_buf());
    }
    None
}

fn resolve_engine() -> Option<EnginePaths> {
    let env = GateEnv::from_process();
    assert!(
        env.requested(),
        "{JOB} was invoked without {NATIVE_GATE_ENV}=1; this test is a required-native job \
         and must never run as a default pass"
    );
    let required = env.required();
    let mut executable = env.engine_bin.map(PathBuf::from);
    if executable.is_none()
        && let Some(package) = env.package.map(PathBuf::from)
    {
        executable = Some(package.join("engine/lib/tsc"));
    }
    let Some(executable) = executable else {
        assert!(
            !required,
            "{JOB}: {TSGO_BIN_ENV} is required when {REQUIRE_REAL_ENGINE_ENV}=1; \
             set it to the pinned {ENGINE_VERSION} engine (sha256 {sha})",
            sha = PinnedEngine::pinned().executable_sha256()
        );
        eprintln!(
            "SKIP({JOB}): {TSGO_BIN_ENV} is not set; explicit optional-mode skip, not evidence"
        );
        return None;
    };
    assert!(
        executable.is_file(),
        "{JOB}: {TSGO_BIN_ENV} is not a file: {}",
        executable.display()
    );
    let bytes = std::fs::read(&executable)
        .unwrap_or_else(|error| panic!("read engine {}: {error}", executable.display()));
    let actual = sha256_hex(&bytes);
    let pinned = PinnedEngine::pinned().executable_sha256();
    assert_eq!(
        actual,
        pinned,
        "{JOB}: engine {} is not the pinned artifact",
        executable.display()
    );
    let version = Command::new(&executable)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("run engine --version: {error}"));
    assert!(
        version.status.success(),
        "{JOB}: engine --version failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    let stdout = String::from_utf8_lossy(&version.stdout);
    assert!(
        stdout.contains(ENGINE_VERSION_LINE) || stdout.contains(ENGINE_VERSION),
        "{JOB}: engine --version {stdout:?} does not identify {ENGINE_VERSION}"
    );

    let asset_root = env
        .asset_root
        .map(PathBuf::from)
        .or_else(|| executable.parent().map(Path::to_path_buf));
    let Some(asset_root) = asset_root.and_then(|root| resolve_standard_lib_root(&root)) else {
        assert!(
            !required,
            "{JOB}: {TSGO_ASSET_ROOT_ENV} (or the engine's lib directory) must contain lib.d.ts"
        );
        eprintln!(
            "SKIP({JOB}): no pinned standard library near {TSGO_BIN_ENV}; explicit optional-mode \
             skip, not evidence"
        );
        return None;
    };

    let typings = env.typings_root.map(PathBuf::from).or_else(|| {
        let candidate = workspace().join("backends/typescript/node_modules");
        candidate.is_dir().then_some(candidate)
    });
    let Some(typings_root) = typings.and_then(|root| resolve_typings_root(&root)) else {
        assert!(
            !required,
            "{JOB}: {TSGO_TYPINGS_ROOT_ENV} must contain miniprogram-api-typings 5.3.1"
        );
        eprintln!(
            "SKIP({JOB}): no pinned miniprogram typings root; explicit optional-mode skip, \
             not evidence"
        );
        return None;
    };

    Some(EnginePaths {
        executable,
        asset_root,
        typings_root,
    })
}

fn load_assets(engine: &EnginePaths) -> Arc<TypeAssets> {
    let assets =
        TypeAssets::load_pinned(&engine.asset_root, &engine.typings_root).unwrap_or_else(|error| {
            panic!(
                "{JOB}: TypeAssets::load_pinned failed (fail closed, no noLib fallback): {error}"
            )
        });
    assert_eq!(
        assets.files().len(),
        EXPECTED_ASSET_FILES,
        "{JOB}: 116 lib + 20 typings"
    );
    Arc::new(assets)
}

fn new_work_root() -> TestRoot {
    let env = GateEnv::from_process();
    TestRoot::new(&env, JOB)
}

fn assert_top_level_table(projection: &Projection) {
    assert_eq!(
        projection.top_level_cases.len(),
        10,
        "{JOB}: top-level case table"
    );
    let exact: Vec<&str> = projection
        .top_level_cases
        .iter()
        .filter(|case| case.in_place_half == "exact")
        .map(|case| case.case_id.as_str())
        .collect();
    assert_eq!(
        exact,
        [
            "TS-MATRIX-0092",
            "TS-MATRIX-0228",
            "TS-MATRIX-0301",
            "TS-MATRIX-0319"
        ],
        "{JOB}: in-place exact top-level halves"
    );
    for case in &projection.top_level_cases {
        assert!(
            !case.title.is_empty(),
            "{JOB}: top-level case {} title",
            case.case_id
        );
        if case.in_place_half == "blocked" {
            assert!(
                case.blocked_by.is_some() && case.note.is_some(),
                "{JOB}: blocked top-level case {} must name blockedBy and a note",
                case.case_id
            );
        }
    }
}

#[test]
fn real_matrix_manifest_is_frozen_and_oracle_derived() {
    let value = load_projection_value();
    let projection: Projection = serde_json::from_value(value.clone()).expect("projection");
    assert_eq!(projection.schema, "tsgo-real-matrix-projection/1");
    assert_eq!(projection.source.path, ORACLE_RELATIVE_PATH);
    assert_eq!(projection.source.sha256, ORACLE_SHA256);
    assert_eq!(projection.source.size, ORACLE_SIZE);
    assert_eq!(projection.source.selection.layout, "in-place-shadow");
    assert_eq!(projection.source.selection.scenarios, SCENARIOS);
    assert_eq!(projection.scenarios.len(), SCENARIOS);
    assert_eq!(projection.source.selection.in_place_diagnostics, 17);

    let oracle_path = workspace().join(ORACLE_RELATIVE_PATH);
    let oracle_bytes = std::fs::read(&oracle_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", oracle_path.display()));
    assert_eq!(sha256_hex(&oracle_bytes), ORACLE_SHA256);
    assert_eq!(
        u64::try_from(oracle_bytes.len()).expect("size"),
        ORACLE_SIZE
    );
    assert_eq!(
        projection.source.environment["typescript"],
        json!("6.0.3"),
        "{JOB}: the projection must record the old oracle environment"
    );
    assert_eq!(projection.source.environment["typings"], json!("5.3.1"));

    let scenario_source = workspace().join(&projection.source.scenario_manifest.path);
    let scenario_bytes = std::fs::read(&scenario_source)
        .unwrap_or_else(|error| panic!("read {}: {error}", scenario_source.display()));
    assert_eq!(
        sha256_hex(&scenario_bytes),
        projection.source.scenario_manifest.sha256
    );

    let scenarios_value = value.get("scenarios").expect("scenarios");
    assert_eq!(
        sha256_hex(
            serde_json::to_string(scenarios_value)
                .expect("canonical")
                .as_bytes()
        ),
        projection.digest,
        "{JOB}: projection digest drift"
    );

    let mut exact = 0usize;
    let mut blocked = BTreeMap::<String, usize>::new();
    let mut ids = BTreeSet::new();
    for scenario in &projection.scenarios {
        assert!(ids.insert(scenario.id.clone()), "duplicate scenario id");
        let expected_case = format!(
            "TS-MATRIX-0102-{}-IN_PLACE_SHADOW",
            scenario.id.to_ascii_uppercase().replace('-', "_")
        );
        assert_eq!(
            scenario.case_id, expected_case,
            "scenario {}: ledger target id",
            scenario.id
        );
        match scenario.classification.status.as_str() {
            "exact" => {
                assert!(
                    scenario.classification.blocked_by.is_none(),
                    "scenario {}: exact case must not name blockedBy",
                    scenario.id
                );
                exact += 1;
            }
            "blocked" => {
                let category = scenario
                    .classification
                    .blocked_by
                    .as_deref()
                    .unwrap_or_else(|| {
                        panic!("scenario {}: blocked without category", scenario.id)
                    });
                *blocked.entry(category.to_owned()).or_default() += 1;
            }
            other => panic!("scenario {}: unknown status {other}", scenario.id),
        }

        assert_eq!(
            scenario.fragments.len(),
            scenario.oracle.request.fragment_count,
            "scenario {}: request fragmentCount",
            scenario.id
        );
        assert_eq!(
            scenario
                .compiler_options
                .get("allowJs")
                .and_then(Value::as_bool),
            Some(scenario.allow_js),
            "scenario {}: allowJs in the frozen options",
            scenario.id
        );
        assert_eq!(
            scenario
                .compiler_options
                .get("checkJs")
                .and_then(Value::as_bool),
            Some(scenario.check_js),
            "scenario {}: checkJs in the frozen options",
            scenario.id
        );
        assert!(
            matches!(scenario.js_check.as_str(), "inherit" | "pragma" | "off"),
            "scenario {}: jsCheck policy",
            scenario.id
        );
        assert!(
            matches!(scenario.oracle.module_before.as_str(), "script" | "module"),
            "scenario {}: moduleBefore",
            scenario.id
        );
        assert!(
            matches!(scenario.oracle.module_after.as_str(), "script" | "module"),
            "scenario {}: moduleAfter",
            scenario.id
        );
        for (capability, state) in &scenario.oracle.states {
            if state.status != "supported" {
                assert!(
                    state.reason.is_some(),
                    "scenario {}: {capability} degradation must carry a reason",
                    scenario.id
                );
            }
        }
        for query in &scenario.queries {
            if let Some(target) = &query.target {
                let fragment = scenario
                    .fragments
                    .iter()
                    .find(|fragment| fragment.expr == query.expr)
                    .unwrap_or_else(|| panic!("scenario {}: query fragment", scenario.id));
                assert_eq!(
                    fragment.target.as_deref(),
                    Some(target.as_str()),
                    "scenario {}: frozen fragment target for {}",
                    scenario.id,
                    query.expr
                );
            }
            if query.definition {
                let observation = scenario
                    .oracle
                    .definitions
                    .iter()
                    .find(|definition| definition.query == query.expr)
                    .unwrap_or_else(|| panic!("scenario {}: oracle definition", scenario.id));
                assert_eq!(
                    observation
                        .observation
                        .as_ref()
                        .map(|observation| observation.kind.as_str()),
                    query.expect_definition.as_deref(),
                    "scenario {}: oracle definition kind for {}",
                    scenario.id,
                    query.expr
                );
            }
        }
        for hover in &scenario.oracle.hovers {
            let Some(observation) = &hover.observation else {
                continue;
            };
            assert!(
                observation.documentation.is_some(),
                "scenario {}: oracle hover documentation for {}",
                scenario.id,
                hover.query
            );
            if let Some(source) = &observation.documentation_source {
                assert!(
                    matches!(source.as_str(), "quickinfo" | "copied" | "none"),
                    "scenario {}: documentationSource",
                    scenario.id
                );
            }
            if let Some(quality) = &observation.quality {
                assert!(
                    matches!(quality.as_str(), "known" | "missing" | "any" | "unknown"),
                    "scenario {}: hover quality",
                    scenario.id
                );
            }
        }
        if scenario.classification.blocked_by.as_deref() == Some("docs") {
            assert!(
                scenario
                    .queries
                    .iter()
                    .any(|query| query.expect_doc_source.is_some()),
                "scenario {}: a DOCS case must freeze documentationSource here",
                scenario.id
            );
        }
        assert_eq!(
            scenario.oracle.request.owner_kind, scenario.registration,
            "scenario {}: request ownerKind",
            scenario.id
        );
        assert!(
            scenario.oracle.request.file_count >= 1,
            "scenario {}: request fileCount",
            scenario.id
        );
        let mut expressions = BTreeSet::new();
        for fragment in &scenario.fragments {
            assert!(
                expressions.insert(fragment.expr.clone()),
                "scenario {}: duplicate fragment expression",
                scenario.id
            );
            assert_eq!(
                fragment.wxml_range.end - fragment.wxml_range.start,
                u32::try_from(fragment.expr.len()).expect("expr len"),
                "scenario {}: fragment range must match UTF-8 expression length",
                scenario.id
            );
            let bytes = scenario.wxml.as_bytes();
            assert_eq!(
                &bytes[fragment.wxml_range.start as usize..fragment.wxml_range.end as usize],
                fragment.expr.as_bytes(),
                "scenario {}: fragment range drift",
                scenario.id
            );
            assert_eq!(
                locate_expression(&scenario.wxml, &fragment.expr, &scenario.id),
                fragment.wxml_range,
                "scenario {}: fragment range is not locateExpression output",
                scenario.id
            );
        }
        assert_eq!(
            scenario.oracle.diagnostics.len(),
            scenario.expected_diagnostics.len(),
            "scenario {}: oracle diagnostic count",
            scenario.id
        );
        for expected in &scenario.expected_diagnostics {
            let expression = locate_expression(&scenario.wxml, &expected.expr, &scenario.id);
            let target_offset = expected
                .expr
                .find(&expected.target)
                .unwrap_or_else(|| panic!("scenario {}: target not in expression", scenario.id));
            let recomputed = byte_range(
                &scenario.wxml,
                expression.start as usize + target_offset,
                expected.target.len(),
            );
            assert_eq!(
                recomputed, expected.range,
                "scenario {}: expected diagnostic range drift",
                scenario.id
            );
            assert!(
                scenario
                    .oracle
                    .diagnostics
                    .iter()
                    .any(|diagnostic| { diagnostic.range == expected.range }),
                "scenario {}: expected range missing from the oracle projection",
                scenario.id
            );
        }
        assert_eq!(
            expected_diagnostic_tuples(scenario),
            scenario
                .expected_diagnostics
                .iter()
                .map(|expected| {
                    let code = scenario
                        .oracle
                        .diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.range == expected.range)
                        .unwrap_or_else(|| {
                            panic!(
                                "scenario {}: oracle diagnostic missing for expected range",
                                scenario.id
                            )
                        })
                        .code;
                    (code, expected.range.start, expected.range.end)
                })
                .collect::<BTreeSet<_>>(),
            "scenario {}: expected diagnostics must match the oracle projection",
            scenario.id
        );
        assert!(
            scenario.oracle.diagnostics_exact,
            "scenario {}: the old oracle must be exact",
            scenario.id
        );
        assert!(
            !scenario.oracle.perturbs_user_types,
            "scenario {}: the old oracle records no perturbation",
            scenario.id
        );
    }
    assert_eq!(exact, EXACT_CASES, "{JOB}: exact in-place cases");
    assert_eq!(exact + blocked.values().sum::<usize>(), SCENARIOS);
    assert_eq!(
        blocked,
        BTreeMap::from([
            ("docs".to_owned(), 3),
            ("checker".to_owned(), 1),
            ("probe".to_owned(), 1),
            ("root".to_owned(), 1),
            ("state".to_owned(), 5),
        ]),
        "{JOB}: blocked categories"
    );

    assert_eq!(
        projection.source.sample_files.len(),
        33,
        "{JOB}: 32 frozen sample files"
    );
    let sample_root = workspace().join(SAMPLES_RELATIVE_PATH);
    let mut disk_files = BTreeSet::new();
    collect_files(&sample_root, &sample_root, &mut disk_files);
    let frozen: BTreeSet<String> = projection.source.sample_files.keys().cloned().collect();
    assert_eq!(
        disk_files, frozen,
        "{JOB}: frozen sample set must match disk"
    );
    for (relative, expected) in &projection.source.sample_files {
        let path = sample_root.join(relative);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("read sample {}: {error}", path.display()));
        assert_eq!(
            u64::try_from(bytes.len()).expect("size"),
            expected.size,
            "{JOB}: sample size {relative}"
        );
        assert_eq!(
            sha256_hex(&bytes),
            expected.sha256,
            "{JOB}: sample digest {relative}"
        );
    }
    for scenario in &projection.scenarios {
        let mut declared = BTreeSet::new();
        declared.insert(format!(
            "{}/{}",
            scenario.sample_dir,
            scenario.scenario_script()
        ));
        for extra in &scenario.extra_files {
            declared.insert(format!("{}/{}", scenario.sample_dir, extra));
        }
        for path in &declared {
            assert!(
                projection.source.sample_files.contains_key(path),
                "scenario {}: sample {path} is not frozen",
                scenario.id
            );
        }
    }

    let forbidden = [
        "observedAt",
        "observed_at",
        "nativeEngine",
        "native_engine",
        "bless",
        "updateExpect",
        "instaUpdate",
    ];
    for key in forbidden {
        assert!(
            !json_contains_key(&value, key),
            "{JOB}: the projection must not record native replay field {key}"
        );
    }

    assert_top_level_table(&projection);
}

impl Scenario {
    fn scenario_script(&self) -> &str {
        &self.script
    }
}

fn collect_files(root: &Path, directory: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.is_file() {
            out.insert(
                path.strip_prefix(root)
                    .expect("relative")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

fn json_contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.keys().any(|candidate| candidate == key)
                || map.values().any(|value| json_contains_key(value, key))
        }
        Value::Array(items) => items.iter().any(|item| json_contains_key(item, key)),
        _ => false,
    }
}

#[test]
fn real_matrix_comparator_rejects_missing_shifted_and_perturbed_results() {
    let base = BTreeSet::from([(2339, 134, 138)]);
    assert!(
        compare_diagnostics(&base, &base, 1).exact(),
        "identical diagnostics must pass"
    );
    assert!(
        !compare_diagnostics(&base, &BTreeSet::new(), 1).exact(),
        "a missing diagnostic must fail"
    );
    assert!(
        !compare_diagnostics(&BTreeSet::new(), &base, 1).exact(),
        "an unexpected diagnostic must fail"
    );
    let shifted = BTreeSet::from([(2339, 135, 139)]);
    assert!(
        !compare_diagnostics(&base, &shifted, 1).exact(),
        "a one-byte range shift must fail"
    );
    let different_code = BTreeSet::from([(2345, 134, 138)]);
    assert!(
        !compare_diagnostics(&base, &different_code, 1).exact(),
        "the same range with a different code must fail"
    );
    assert!(
        !compare_diagnostics(&BTreeSet::new(), &BTreeSet::new(), 0).exact(),
        "empty versus empty is not evidence when no scenario ran"
    );
    assert!(
        compare_diagnostics(&BTreeSet::new(), &BTreeSet::new(), EXACT_CASES).exact(),
        "empty versus empty passes only when scenarios ran"
    );

    let expected = BTreeSet::from([("string".to_owned(), Some("页面标题".to_owned()))]);
    let observed = expected.clone();
    assert!(compare_hover_docs(&expected, &observed), "identical hovers");
    let lost_doc = BTreeSet::from([("string".to_owned(), None)]);
    assert!(
        !compare_hover_docs(&expected, &lost_doc),
        "a lost hover documentation must fail"
    );

    let baseline = Fingerprint {
        offsets: vec![42],
        types: vec!["string".to_owned()],
    };
    let perturbed = Fingerprint {
        offsets: vec![42],
        types: vec!["number".to_owned()],
    };
    assert!(fingerprint_perturbs(&baseline, &perturbed));
    assert!(!fingerprint_perturbs(&baseline, &baseline));
    assert!(fingerprint_is_meaningful(&baseline));
    let empty = Fingerprint {
        offsets: Vec::new(),
        types: Vec::new(),
    };
    assert!(
        !fingerprint_is_meaningful(&empty),
        "an empty fingerprint must not be treated as meaningful equality"
    );
    let missing = Fingerprint {
        offsets: vec![42],
        types: vec!["<none>".to_owned()],
    };
    assert!(
        !fingerprint_is_meaningful(&missing),
        "an unmeasured fingerprint must not be treated as meaningful equality"
    );

    let baseline_keys = vec![(2322, 10u32, 3u32)];
    let mounts: Vec<MountRecord> = Vec::new();
    let map = empty_map();
    let same = vec![raw_diagnostic(2322, 10, 13)];
    assert!(
        introduced_codes(&baseline_keys, &same, SHADOW_TEXT, &mounts, &map).is_empty(),
        "a matching baseline key must not be introduced"
    );
    let extra = vec![raw_diagnostic(2322, 20, 23)];
    assert_eq!(
        introduced_codes(&baseline_keys, &extra, SHADOW_TEXT, &mounts, &map),
        vec![2322],
        "a different key must be introduced"
    );
}

const SHADOW_TEXT: &str = "012345678901234567890123456789";

fn raw_diagnostic(code: u32, start: u32, end: u32) -> NativeDiagnostic {
    NativeDiagnostic {
        code,
        message: String::new(),
        severity: DiagnosticSeverity::Error,
        start: position_of(SHADOW_TEXT, start),
        end: position_of(SHADOW_TEXT, end),
    }
}

fn empty_map() -> wxml_backend_tsgo::shadow::mapping::ShadowMap {
    let assembled = Assembled {
        text: SHADOW_TEXT.to_owned(),
        edits: Vec::new(),
    };
    wxml_backend_tsgo::shadow::mapping::ShadowMap::new(SHADOW_TEXT, &assembled)
}

fn compare_hover_docs(
    expected: &BTreeSet<(String, Option<String>)>,
    observed: &BTreeSet<(String, Option<String>)>,
) -> bool {
    expected == observed
}

#[test]
#[ignore = "required native matrix job: WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1"]
fn real_matrix_in_place_matches_frozen_oracle() {
    let Some(engine) = resolve_engine() else {
        return;
    };
    let assets = load_assets(&engine);
    let projection = load_projection();
    let work = new_work_root();
    let mut exact = 0usize;
    let mut blocked = 0usize;
    let mut sentinels = BTreeMap::<String, BTreeSet<(u32, u32, u32)>>::new();
    let mut observed_by_id = BTreeMap::new();
    for (index, scenario) in projection.scenarios.iter().enumerate() {
        assert_query_expectations(scenario);
        let observation = observe_scenario(scenario, &engine, &assets, &work, index);
        assert_states(scenario, &observation);
        assert_diagnostics(scenario, &observation);
        assert_hover_facts(scenario, &observation);
        assert_definition_facts(scenario, &observation);
        assert_non_interference(scenario, &observation);
        match scenario.classification.status.as_str() {
            "exact" => {
                exact += 1;
                assert_eq!(
                    observation.diagnostics,
                    expected_diagnostic_tuples(scenario),
                    "scenario {}: exact diagnostics",
                    scenario.id
                );
            }
            "blocked" => {
                blocked += 1;
                assert_gap_expectation(scenario, &observation);
                let gap = scenario
                    .classification
                    .blocked_by
                    .as_deref()
                    .unwrap_or("unknown");
                eprintln!(
                    "blocked({gap}): {} checker diagnostics={} introduced={:?}",
                    scenario.id,
                    observation.diagnostics.len(),
                    observation.introduced
                );
            }
            other => panic!("scenario {}: unknown status {other}", scenario.id),
        }
        if !observation.diagnostics.is_empty() {
            sentinels.insert(scenario.id.clone(), observation.diagnostics.clone());
        }
        eprintln!(
            "scenario {} baseline={} suppressed={} unmapped={} introduced={:?}",
            scenario.id,
            observation.baseline_diagnostics,
            observation.suppressed,
            observation.unmapped,
            observation.introduced
        );
        observed_by_id.insert(scenario.id.clone(), observation);
    }
    assert_eq!(exact, EXACT_CASES, "{JOB}: exact in-place cases");
    assert_eq!(blocked, BLOCKED_CASES, "{JOB}: blocked in-place cases");
    assert_eq!(observed_by_id.len(), SCENARIOS);

    assert_sentinel(&sentinels, "nullable", &[(18047, 45, 54), (18046, 98, 106)]);
    assert_sentinel(&sentinels, "unchecked-index", &[(2532, 44, 52)]);
    assert_sentinel(&sentinels, "js-shebang-bom", &[(2339, 91, 98)]);
    assert_sentinel(&sentinels, "js-source-check-inherit", &[(2339, 91, 98)]);
    assert_sentinel(&sentinels, "js-bom-check-inherit", &[(2339, 91, 98)]);
    assert_sentinel(&sentinels, "unicode-crlf", &[(2339, 103, 107)]);
    assert_sentinel(&sentinels, "js-jsdoc-pragma", &[(2339, 140, 144)]);
    let jsdoc = &observed_by_id["js-jsdoc-pragma"];
    assert_eq!(
        jsdoc.introduced,
        vec![2322],
        "js-jsdoc-pragma: positive introduced sentinel"
    );

    assert_top_level_halves(&observed_by_id, &projection);

    println!(
        "{SUMMARY_PREFIX} exact={exact} gaps={blocked} scenarios={} engine={ENGINE_VERSION} \
         oracle={ORACLE_SHA256} projection={}",
        projection.scenarios.len(),
        projection.digest
    );
}

fn assert_sentinel(
    sentinels: &BTreeMap<String, BTreeSet<(u32, u32, u32)>>,
    scenario: &str,
    expected: &[(u32, u32, u32)],
) {
    let observed = sentinels
        .get(scenario)
        .unwrap_or_else(|| panic!("{JOB}: positive sentinel {scenario} was not observed"));
    for entry in expected {
        assert!(
            observed.contains(entry),
            "{JOB}: sentinel {scenario} must observe {entry:?}; got {observed:?}"
        );
    }
}

fn assert_top_level_halves(
    observed: &BTreeMap<String, ScenarioObservation>,
    projection: &Projection,
) {
    assert_eq!(
        observed.len(),
        SCENARIOS,
        "TS-MATRIX-0092: every in-place scenario must build without throwing"
    );
    let data_shorthand = &observed["data-shorthand"];
    let BuiltFacts::Unsupported { reason, .. } = &data_shorthand.built else {
        panic!("TS-MATRIX-0228: data-shorthand must be Unsupported");
    };
    assert!(
        reason.contains("not a property assignment"),
        "TS-MATRIX-0228: {reason:?}"
    );
    assert!(
        !reason.contains("no `data` member"),
        "TS-MATRIX-0228: a shorthand data member is not a missing data member"
    );

    let multi = &observed["multi-registration"];
    let BuiltFacts::Built {
        reason,
        registration_count,
        ..
    } = &multi.built
    else {
        panic!("TS-MATRIX-0301: multi-registration must build partially");
    };
    assert!(
        reason
            .as_deref()
            .is_some_and(|reason| reason.contains("2 page registrations")),
        "TS-MATRIX-0301: {reason:?}"
    );
    assert_eq!(*registration_count, 2, "TS-MATRIX-0301: registration count");
    assert_eq!(
        multi.hovers["title"].type_text, "string",
        "TS-MATRIX-0301: the first registration wins"
    );

    assert!(
        observed["unchecked-index"]
            .diagnostics
            .contains(&(2532, 44, 52)),
        "TS-MATRIX-0319: noUncheckedIndexedAccess must produce 2532"
    );
    let nullable_off = &observed["nullable-no-strict-null"];
    assert!(
        !nullable_off
            .diagnostics
            .iter()
            .any(|(code, _, _)| *code == 18047),
        "TS-MATRIX-0319: strictNullChecks:false must not produce 18047"
    );
    assert_eq!(
        nullable_off.hovers["maybeUser.name"].type_text, "string",
        "TS-MATRIX-0319: maybeUser.name stays string"
    );

    let blocked: Vec<&str> = projection
        .top_level_cases
        .iter()
        .filter(|case| case.in_place_half == "blocked")
        .map(|case| case.case_id.as_str())
        .collect();
    assert_eq!(
        blocked,
        [
            "TS-MATRIX-0215",
            "TS-MATRIX-0239",
            "TS-MATRIX-0252",
            "TS-MATRIX-0271",
            "TS-MATRIX-0283",
            "TS-MATRIX-0335"
        ],
        "top-level blocked list"
    );
}

#[test]
#[ignore = "required native matrix job: WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1"]
fn real_matrix_known_gaps_are_characterized() {
    let Some(engine) = resolve_engine() else {
        return;
    };
    let assets = load_assets(&engine);
    let projection = load_projection();
    let work = new_work_root();
    let mut gaps = 0usize;
    for (index, scenario) in projection.scenarios.iter().enumerate() {
        if scenario.classification.status != "blocked" {
            continue;
        }
        gaps += 1;
        let observation = observe_scenario(scenario, &engine, &assets, &work, 100 + index);
        assert_diagnostics(scenario, &observation);
        assert_hover_facts(scenario, &observation);
        assert_definition_facts(scenario, &observation);
        assert_non_interference(scenario, &observation);
        assert_gap_expectation(scenario, &observation);
        eprintln!(
            "blocked({}): {} build={} diagnostics={} hover_doc_source={}",
            scenario
                .classification
                .blocked_by
                .as_deref()
                .unwrap_or("unknown"),
            scenario.id,
            match &observation.built {
                BuiltFacts::Unsupported { .. } => "unsupported",
                BuiltFacts::Built { status, .. } => build_status_name(*status),
            },
            observation.diagnostics.len(),
            scenario
                .queries
                .iter()
                .filter(|query| query.expect_doc_source.is_some())
                .count()
        );
    }
    assert_eq!(gaps, BLOCKED_CASES, "{JOB}: characterized gaps");
    println!(
        "{SUMMARY_PREFIX} gaps={gaps} scenarios={} engine={ENGINE_VERSION} oracle={ORACLE_SHA256}",
        projection.scenarios.len()
    );
}

#[test]
#[ignore = "required native matrix job: WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1"]
fn real_matrix_top_level_cases() {
    let Some(engine) = resolve_engine() else {
        return;
    };
    let assets = load_assets(&engine);
    let projection = load_projection();
    assert_top_level_table(&projection);
    let work = new_work_root();

    let required = [
        "data-shorthand",
        "multi-registration",
        "unchecked-index",
        "nullable-no-strict-null",
    ];
    let mut observed = BTreeMap::new();
    let mut next_index = 200usize;
    for scenario in &projection.scenarios {
        if !required.contains(&scenario.id.as_str()) {
            continue;
        }
        observed.insert(
            scenario.id.clone(),
            observe_scenario(scenario, &engine, &assets, &work, next_index),
        );
        next_index += 1;
    }
    assert_eq!(observed.len(), required.len());
    let data_shorthand = &observed["data-shorthand"];
    let BuiltFacts::Unsupported { reason, .. } = &data_shorthand.built else {
        panic!("TS-MATRIX-0228: data-shorthand must be Unsupported");
    };
    assert!(
        reason.contains("not a property assignment"),
        "TS-MATRIX-0228: {reason:?}"
    );
    let multi = &observed["multi-registration"];
    let BuiltFacts::Built {
        reason,
        registration_count,
        ..
    } = &multi.built
    else {
        panic!("TS-MATRIX-0301: multi-registration must build partially");
    };
    assert!(
        reason
            .as_deref()
            .is_some_and(|reason| reason.contains("2 page registrations")),
        "TS-MATRIX-0301: {reason:?}"
    );
    assert_eq!(*registration_count, 2);
    assert_eq!(multi.hovers["title"].type_text, "string");
    assert!(
        observed["unchecked-index"]
            .diagnostics
            .contains(&(2532, 44, 52)),
        "TS-MATRIX-0319: 2532 was not observed"
    );
    assert!(
        observed["nullable-no-strict-null"].diagnostics.is_empty(),
        "TS-MATRIX-0319: strictNullChecks:false must stay clean"
    );
    assert_eq!(
        observed["nullable-no-strict-null"].hovers["maybeUser.name"].type_text,
        "string"
    );
    println!(
        "{SUMMARY_PREFIX} top_level_in_place_exact=4 top_level_blocked=6 engine={ENGINE_VERSION}"
    );
}
