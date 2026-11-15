//! Strict structural validation of the shared acceptance fixtures (schema v2).
//!
//! These tests do not analyze WXML. They verify that the manifest is complete,
//! every referenced path stays inside its fixture, every marker is unique and
//! locatable, targets point at the right token, expected documents have the v2
//! shape, and CRLF fixtures are byte-exact.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};
use wxml_fixtures::{fixtures_dir, manifest_path};
use wxml_text::LineIndex;

const MANIFEST_VERSION: u64 = 2;

const ACCEPTANCE_CRITERIA: [&str; 13] = [
    "AC-001", "AC-002", "AC-003", "AC-004", "AC-005", "AC-006", "AC-007", "AC-008", "AC-009",
    "AC-010", "AC-011", "AC-012", "AC-013",
];

const TYPE_KINDS: [&str; 10] = [
    "string", "number", "boolean", "any", "unknown", "union", "object", "alias", "missing", "other",
];

const STATUSES: [&str; 6] = [
    "supported",
    "partial",
    "unsupported",
    "disabled",
    "unavailable",
    "unknown",
];

const QUALITIES: [&str; 4] = ["known", "any", "unknown", "missing"];

const SEVERITIES: [&str; 4] = ["error", "warning", "information", "hint"];

const QUERY_KEYS: [&str; 5] = ["file", "marker", "target", "position", "expect"];

const EXPECT_KEYS: [&str; 6] = [
    "type",
    "typeKind",
    "documentation",
    "status",
    "quality",
    "rangeTarget",
];

const DIAGNOSTIC_KEYS: [&str; 5] = ["file", "marker", "target", "severity", "messageContains"];

type Check<T = ()> = Result<T, String>;

fn require_only_keys(object: &Map<String, Value>, allowed: &[&str], context: &str) -> Check {
    for key in object.keys() {
        require(
            allowed.contains(&key.as_str()),
            format!("{context}: unknown field `{key}`"),
        )?;
    }
    Ok(())
}

fn require(condition: bool, message: impl Into<String>) -> Check {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn object<'a>(value: &'a Value, context: &str) -> Check<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))
}

fn field<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Check<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| format!("{context}: missing `{key}`"))
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Check<&'a str> {
    field(object, key, context)?
        .as_str()
        .ok_or_else(|| format!("{context}: `{key}` must be a string"))
}

fn array_field<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> Check<&'a [Value]> {
    field(object, key, context)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context}: `{key}` must be an array"))
}

fn number_field(object: &Map<String, Value>, key: &str, context: &str) -> Check<u64> {
    field(object, key, context)?
        .as_u64()
        .ok_or_else(|| format!("{context}: `{key}` must be a non-negative integer"))
}

fn optional_enum(object: &Map<String, Value>, key: &str, allowed: &[&str], context: &str) -> Check {
    match object.get(key) {
        None => Ok(()),
        Some(Value::String(value)) => require(
            allowed.contains(&value.as_str()),
            format!("{context}: unknown `{key}` value `{value}`"),
        ),
        Some(_) => Err(format!("{context}: `{key}` must be a string")),
    }
}

/// Counts markers, preserving duplicates across different files.
///
/// A marker is unique inside one file, but the same expression can legitimately
/// appear in the LF and CRLF variants of a sample. The manifest must list such
/// markers once per occurrence so deleting either entry is detected.
fn marker_multiset(
    case: &Map<String, Value>,
    key: &str,
    id: &str,
) -> Check<BTreeMap<String, usize>> {
    let context = format!("case `{id}`");
    let mut markers = BTreeMap::new();
    for (index, value) in array_field(case, key, &context)?.iter().enumerate() {
        let marker = value
            .as_str()
            .ok_or_else(|| format!("{context}: `{key}[{index}]` must be a string"))?;
        require(
            !marker.is_empty(),
            format!("{context}: `{key}[{index}]` must not be empty"),
        )?;
        *markers.entry(marker.to_owned()).or_insert(0) += 1;
    }
    Ok(markers)
}

fn is_source_file(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("wxml" | "ts" | "js")
    )
}

fn is_wxml_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        == Some("wxml")
}

fn resolve_inside(root: &Path, relative: &str) -> Check<PathBuf> {
    require(!relative.is_empty(), "fixture path must not be empty")?;
    let candidate = Path::new(relative);
    require(
        !candidate.is_absolute(),
        format!("fixture path `{relative}` must be relative"),
    )?;
    require(
        !candidate.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }),
        format!("fixture path `{relative}` must not contain `..` or a root prefix"),
    )?;
    let joined = root.join(candidate);
    let canonical = joined
        .canonicalize()
        .map_err(|error| format!("failed to resolve `{relative}`: {error}"))?;
    require(
        canonical.starts_with(root),
        format!("fixture path `{relative}` escapes the fixture root"),
    )?;
    Ok(canonical)
}

fn read_text(path: &Path) -> Check<String> {
    fs::read_to_string(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
}

fn read_json(path: &Path) -> Check<Value> {
    serde_json::from_str(&read_text(path)?)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn validate_marker(text: &str, marker: &str, context: &str) -> Check {
    require(
        !marker.is_empty(),
        format!("{context}: marker must not be empty"),
    )?;
    let count = text.matches(marker).count();
    require(
        count == 1,
        format!("{context}: marker `{marker}` must occur exactly once, found {count}"),
    )
}

fn target_offset(marker: &str, target: &str, context: &str) -> Check<usize> {
    require(
        !target.is_empty(),
        format!("{context}: target must not be empty"),
    )?;
    let count = marker.matches(target).count();
    require(
        count == 1,
        format!("{context}: target `{target}` must occur exactly once inside the marker"),
    )?;
    marker
        .find(target)
        .ok_or_else(|| format!("{context}: target `{target}` must be a substring of the marker"))
}

/// Computes the UTF-16 position with [`LineIndex`], never with hand-written
/// arithmetic.
fn utf16_position(text: &str, offset: usize, context: &str) -> Check<(u64, u64)> {
    let index = LineIndex::new(text);
    let position = index
        .position(offset)
        .ok_or_else(|| format!("{context}: offset {offset} is not a valid text position"))?;
    Ok((u64::from(position.line), u64::from(position.character)))
}

fn validate_hover(
    document: &Value,
    root: &Path,
    id: &str,
    require_positions: bool,
) -> Check<BTreeMap<String, usize>> {
    let context = format!("case `{id}` hover.json");
    let document = object(document, &context)?;
    let queries = array_field(document, "queries", &context)?;
    require(
        !queries.is_empty(),
        format!("{context}: `queries` must not be empty"),
    )?;

    let mut markers = BTreeMap::new();
    for (index, query) in queries.iter().enumerate() {
        let query_context = format!("{context} query {index}");
        let query = object(query, &query_context)?;
        require_only_keys(query, &QUERY_KEYS, &query_context)?;
        let file = string_field(query, "file", &query_context)?;
        require(
            is_wxml_file(file),
            format!("{query_context}: hover file `{file}` must be a .wxml file"),
        )?;
        let text = read_text(&resolve_inside(root, file)?)?;

        let marker = string_field(query, "marker", &query_context)?;
        validate_marker(&text, marker, &query_context)?;
        let target = string_field(query, "target", &query_context)?;
        let target_in_marker = target_offset(marker, target, &query_context)?;

        let expect_context = format!("{query_context} expect");
        let expect = object(field(query, "expect", &query_context)?, &expect_context)?;
        require_only_keys(expect, &EXPECT_KEYS, &expect_context)?;
        string_field(expect, "type", &expect_context)?;
        let kind = string_field(expect, "typeKind", &expect_context)?;
        require(
            TYPE_KINDS.contains(&kind),
            format!("{expect_context}: unknown typeKind `{kind}`"),
        )?;
        string_field(expect, "documentation", &expect_context)?;
        optional_enum(expect, "status", &STATUSES, &expect_context)?;
        optional_enum(expect, "quality", &QUALITIES, &expect_context)?;

        if let Some(range_target) = expect.get("rangeTarget") {
            let range_target = range_target
                .as_str()
                .ok_or_else(|| format!("{expect_context}: `rangeTarget` must be a string"))?;
            target_offset(marker, range_target, &expect_context)?;
        }

        let position = query.get("position");
        if require_positions {
            require(
                position.is_some(),
                format!("{query_context}: `position` is required for this case"),
            )?;
        }
        if let Some(position) = position {
            let position_context = format!("{query_context} position");
            let position = object(position, &position_context)?;
            let expected_line = number_field(position, "line", &position_context)?;
            let expected_character = number_field(position, "character", &position_context)?;
            let marker_start = text
                .find(marker)
                .ok_or_else(|| format!("{query_context}: marker is not locatable"))?;
            let (line, character) =
                utf16_position(&text, marker_start + target_in_marker, &position_context)?;
            require(
                line == expected_line && character == expected_character,
                format!(
                    "{position_context}: expected {expected_line}:{expected_character}, computed {line}:{character}"
                ),
            )?;
        }

        *markers.entry(marker.to_owned()).or_insert(0) += 1;
    }
    Ok(markers)
}

struct DiagnosticsSummary {
    markers: BTreeMap<String, usize>,
    status: Option<String>,
}

fn validate_diagnostics(document: &Value, root: &Path, id: &str) -> Check<DiagnosticsSummary> {
    let context = format!("case `{id}` diagnostics.json");
    let document = object(document, &context)?;
    for key in document.keys() {
        require(
            matches!(key.as_str(), "diagnostics" | "status"),
            format!("{context}: unknown field `{key}`"),
        )?;
    }
    let status = match document.get("status") {
        None => None,
        Some(Value::String(value)) => {
            require(
                STATUSES.contains(&value.as_str()),
                format!("{context}: unknown status `{value}`"),
            )?;
            Some(value.clone())
        }
        Some(_) => return Err(format!("{context}: `status` must be a string")),
    };

    let mut markers = BTreeMap::new();
    for (index, entry) in array_field(document, "diagnostics", &context)?
        .iter()
        .enumerate()
    {
        let entry_context = format!("{context} entry {index}");
        let entry = object(entry, &entry_context)?;
        require_only_keys(entry, &DIAGNOSTIC_KEYS, &entry_context)?;
        let file = string_field(entry, "file", &entry_context)?;
        require(
            is_wxml_file(file),
            format!("{entry_context}: diagnostics file `{file}` must be a .wxml file"),
        )?;
        let text = read_text(&resolve_inside(root, file)?)?;
        let marker = string_field(entry, "marker", &entry_context)?;
        validate_marker(&text, marker, &entry_context)?;
        let target = string_field(entry, "target", &entry_context)?;
        target_offset(marker, target, &entry_context)?;
        let severity = string_field(entry, "severity", &entry_context)?;
        require(
            SEVERITIES.contains(&severity),
            format!("{entry_context}: unknown severity `{severity}`"),
        )?;
        let message = string_field(entry, "messageContains", &entry_context)?;
        require(
            !message.is_empty(),
            format!("{entry_context}: `messageContains` must not be empty"),
        )?;
        *markers.entry(marker.to_owned()).or_insert(0) += 1;
    }
    Ok(DiagnosticsSummary { markers, status })
}

fn validate_definition(document: &Value, root: &Path, id: &str) -> Check {
    let context = format!("case `{id}` definition.json");
    let document = object(document, &context)?;
    for key in document.keys() {
        require(
            key == "queries",
            format!("{context}: unknown field `{key}`"),
        )?;
    }
    let queries = array_field(document, "queries", &context)?;
    require(
        !queries.is_empty(),
        format!("{context}: `queries` must not be empty"),
    )?;

    for (index, query) in queries.iter().enumerate() {
        let query_context = format!("{context} query {index}");
        let query = object(query, &query_context)?;
        let file = string_field(query, "file", &query_context)?;
        require(
            is_source_file(file),
            format!("{query_context}: file `{file}` must be .wxml, .ts or .js"),
        )?;
        let text = read_text(&resolve_inside(root, file)?)?;
        let marker = string_field(query, "marker", &query_context)?;
        validate_marker(&text, marker, &query_context)?;
        let target = string_field(query, "target", &query_context)?;
        target_offset(marker, target, &query_context)?;

        match field(query, "expect", &query_context)? {
            Value::Null => {}
            Value::Object(expect) => {
                let expect_context = format!("{query_context} expect");
                let target_file = string_field(expect, "file", &expect_context)?;
                require(
                    is_source_file(target_file),
                    format!("{expect_context}: file `{target_file}` must be .wxml, .ts or .js"),
                )?;
                let target_text = read_text(&resolve_inside(root, target_file)?)?;
                let target_name = string_field(expect, "target", &expect_context)?;
                require(
                    !target_name.is_empty(),
                    format!("{expect_context}: target must not be empty"),
                )?;
                let count = target_text.matches(target_name).count();
                require(
                    count == 1,
                    format!(
                        "{expect_context}: target `{target_name}` must occur exactly once in `{target_file}`, found {count}"
                    ),
                )?;
                let offset = target_text.find(target_name).ok_or_else(|| {
                    format!("{expect_context}: target `{target_name}` is not locatable")
                })?;
                utf16_position(&target_text, offset, &expect_context)?;
            }
            _ => {
                return Err(format!(
                    "{query_context}: `expect` must be null or an object"
                ));
            }
        }
    }
    Ok(())
}

fn validate_fixture_case(case: &Value, fixtures_root: &Path) -> Check {
    let case = object(case, "fixture case")?;
    let id = string_field(case, "id", "fixture case")?;
    let root_relative = string_field(case, "root", id)?;
    let fixtures_root = fixtures_root
        .canonicalize()
        .map_err(|error| format!("fixtures root is not readable: {error}"))?;
    let root = resolve_inside(&fixtures_root, root_relative)
        .map_err(|error| format!("case `{id}`: invalid root `{root_relative}`: {error}"))?;

    let expect_clean = match case.get("expectClean") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(format!("case `{id}`: `expectClean` must be a boolean")),
    };
    let require_positions = match case.get("requirePositions") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(format!("case `{id}`: `requirePositions` must be a boolean")),
    };

    let expected = object(field(case, "expected", id)?, id)?;
    for key in expected.keys() {
        require(
            matches!(key.as_str(), "hover" | "diagnostics" | "definition"),
            format!("case `{id}`: unexpected expected key `{key}`"),
        )?;
    }
    let hover_path = resolve_inside(&root, string_field(expected, "hover", id)?)?;
    let diagnostics_path = resolve_inside(&root, string_field(expected, "diagnostics", id)?)?;

    let positive = marker_multiset(case, "positive", id)?;
    let negative = marker_multiset(case, "negative", id)?;

    let hover_markers = validate_hover(&read_json(&hover_path)?, &root, id, require_positions)?;
    require(
        positive == hover_markers,
        format!("case `{id}`: positive markers must equal the hover.json markers"),
    )?;
    if positive.is_empty() && !expect_clean {
        return Err(format!(
            "case `{id}`: positive markers must not be empty unless expectClean is true"
        ));
    }

    let diagnostics = validate_diagnostics(&read_json(&diagnostics_path)?, &root, id)?;
    require(
        negative == diagnostics.markers,
        format!("case `{id}`: negative markers must equal the diagnostics.json markers"),
    )?;

    match diagnostics.status.as_deref() {
        None | Some("supported") => {
            if expect_clean {
                require(
                    negative.is_empty() && diagnostics.markers.is_empty(),
                    format!(
                        "case `{id}`: expectClean requires empty diagnostics and negative markers"
                    ),
                )?;
            } else {
                require(
                    !negative.is_empty() && !diagnostics.markers.is_empty(),
                    format!("case `{id}`: negative markers and diagnostics must not be empty"),
                )?;
            }
        }
        Some(_) => {
            require(
                !expect_clean,
                format!("case `{id}`: expectClean cannot combine with a degraded status"),
            )?;
            require(
                negative.is_empty() && diagnostics.markers.is_empty(),
                format!("case `{id}`: a degraded diagnostics status requires empty diagnostics"),
            )?;
        }
    }

    if let Some(definition) = expected.get("definition") {
        let definition = definition
            .as_str()
            .ok_or_else(|| format!("case `{id}`: `expected.definition` must be a string"))?;
        validate_definition(&read_json(&resolve_inside(&root, definition)?)?, &root, id)?;
    }
    Ok(())
}

/// AC-012 must keep a definition sample and record the deferred edit coverage.
fn validate_ac012_case(case: &Map<String, Value>, id: &str) -> Check {
    let coverage = object(field(case, "coverage", id)?, id)?;
    string_field(coverage, "edits", id)?;
    let expected = object(field(case, "expected", id)?, id)?;
    require(
        expected.get("definition").is_some(),
        format!("case `{id}`: AC-012 requires expected.definition"),
    )
}

fn validate_runtime_case(case: &Value) -> Check {
    let case = object(case, "runtime case")?;
    let id = string_field(case, "id", "runtime case")?;
    let scenario = string_field(case, "scenario", id)?;
    require(
        !scenario.trim().is_empty(),
        format!("case `{id}`: scenario must not be empty"),
    )?;
    require(
        !scenario.contains("TODO"),
        format!("case `{id}`: scenario must not contain TODO"),
    )?;

    let steps = array_field(case, "steps", id)?;
    require(
        steps.len() >= 2,
        format!("case `{id}`: at least two steps are required"),
    )?;
    for (index, step) in steps.iter().enumerate() {
        let step_context = format!("case `{id}` step {index}");
        let step = object(step, &step_context)?;
        for key in ["action", "expect"] {
            let value = string_field(step, key, &step_context)?;
            require(
                !value.trim().is_empty(),
                format!("{step_context}: `{key}` must not be empty"),
            )?;
            require(
                !value.contains("TODO"),
                format!("{step_context}: `{key}` must not contain TODO"),
            )?;
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &Value, fixtures_root: &Path) -> Check {
    let manifest = object(manifest, "manifest")?;
    require(
        manifest.get("version").and_then(Value::as_u64) == Some(MANIFEST_VERSION),
        format!("manifest must declare version {MANIFEST_VERSION}"),
    )?;

    let cases = array_field(manifest, "cases", "manifest")?;
    require(!cases.is_empty(), "manifest must contain cases")?;

    let mut ids = BTreeSet::new();
    let mut covered = BTreeSet::new();
    let mut runtime_cases = 0;

    for case in cases {
        let case_object = object(case, "manifest case")?;
        let id = string_field(case_object, "id", "manifest case")?;
        require(!id.is_empty(), "manifest case id must not be empty")?;
        require(
            ids.insert(id.to_owned()),
            format!("duplicate case id `{id}`"),
        )?;

        let acs = array_field(case_object, "acs", id)?;
        require(
            !acs.is_empty(),
            format!("case `{id}` must list at least one AC"),
        )?;
        for ac in acs {
            let ac = ac
                .as_str()
                .ok_or_else(|| format!("case `{id}`: AC entries must be strings"))?;
            require(
                ACCEPTANCE_CRITERIA.contains(&ac),
                format!("case `{id}` references unknown criterion `{ac}`"),
            )?;
            covered.insert(ac.to_owned());
        }

        match string_field(case_object, "kind", id)? {
            "fixture" => {
                validate_fixture_case(case, fixtures_root)?;
                if acs.iter().any(|ac| ac.as_str() == Some("AC-012")) {
                    validate_ac012_case(case_object, id)?;
                }
            }
            "runtime" => {
                runtime_cases += 1;
                validate_runtime_case(case)?;
            }
            other => return Err(format!("case `{id}` has unknown kind `{other}`")),
        }
    }

    for ac in ACCEPTANCE_CRITERIA {
        require(covered.contains(ac), format!("no fixture covers {ac}"))?;
    }
    require(
        runtime_cases > 0,
        "manifest must contain at least one runtime case",
    )?;
    Ok(())
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Check {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("failed to read a directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

#[test]
fn manifest_matches_schema_v2() {
    let manifest = read_json(&manifest_path()).expect("manifest parses");
    validate_manifest(&manifest, &fixtures_dir()).expect("fixtures are valid");
}

#[test]
fn wxml_line_endings_are_explicit() {
    let fixtures = fixtures_dir();
    let mut files = Vec::new();
    collect_files(&fixtures, &mut files).expect("fixtures directory is readable");

    let mut crlf_files = 0;
    for path in files {
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("wxml") {
            continue;
        }
        let bytes = fs::read(&path).expect("wxml fixture is readable");
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("");
        if name.ends_with("-crlf.wxml") {
            crlf_files += 1;
            for (index, byte) in bytes.iter().enumerate() {
                if *byte == b'\n' {
                    assert_eq!(
                        bytes.get(index.wrapping_sub(1)),
                        Some(&b'\r'),
                        "{}: every LF in a CRLF fixture must follow CR",
                        path.display()
                    );
                }
            }
            assert!(
                bytes.windows(2).any(|window| window == b"\r\n"),
                "{}: CRLF fixture must contain at least one CRLF",
                path.display()
            );
        } else {
            assert!(
                !bytes.contains(&b'\r'),
                "{}: LF fixture must not contain CR",
                path.display()
            );
        }
    }
    assert!(crlf_files > 0, "at least one CRLF fixture must exist");

    let unicode_crlf = fixtures.join("unicode/pages/index/index-crlf.wxml");
    let bytes = fs::read(unicode_crlf).expect("unicode CRLF fixture exists");
    assert!(bytes.windows(2).any(|window| window == b"\r\n"));
}

/// A temporary fixture tree under the system temp directory.
struct TempFixture {
    path: PathBuf,
}

impl TempFixture {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "wxml-fixtures-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("failed to create parent directory: {error}"));
        }
        fs::write(path, contents)
            .unwrap_or_else(|error| panic!("failed to write fixture: {error}"));
    }
}

impl Drop for TempFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

const VALID_WXML: &str = "<view>{{ user.name }}</view>\n<view>{{ user.nmae }}</view>\n";

fn valid_hover(marker: &str, target: &str) -> Value {
    json!({
        "queries": [{
            "file": "pages/index/index.wxml",
            "marker": marker,
            "target": target,
            "expect": {
                "type": "string",
                "typeKind": "string",
                "documentation": "doc"
            }
        }]
    })
}

fn valid_diagnostics(marker: &str, target: &str) -> Value {
    json!({
        "diagnostics": [{
            "file": "pages/index/index.wxml",
            "marker": marker,
            "target": target,
            "severity": "error",
            "messageContains": target
        }]
    })
}

fn fixture_case(positive: &Value, negative: &Value) -> Value {
    json!({
        "id": "case",
        "kind": "fixture",
        "acs": ["AC-001"],
        "root": "case",
        "positive": positive,
        "negative": negative,
        "expected": {
            "hover": "expected/hover.json",
            "diagnostics": "expected/diagnostics.json"
        }
    })
}

fn setup(wxml: &str, hover: &Value, diagnostics: &Value) -> (TempFixture, Value) {
    let temp = TempFixture::new();
    temp.write("case/pages/index/index.wxml", wxml);
    temp.write("case/expected/hover.json", &hover.to_string());
    temp.write("case/expected/diagnostics.json", &diagnostics.to_string());
    let case = fixture_case(&json!(["{{ user.name }}"]), &json!(["{{ user.nmae }}"]));
    (temp, case)
}

#[test]
fn accepts_a_well_formed_case() {
    let (temp, case) = setup(
        VALID_WXML,
        &valid_hover("{{ user.name }}", "user.name"),
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    validate_fixture_case(&case, temp.path()).expect("case is valid");
}

#[test]
fn rejects_duplicate_markers() {
    let wxml = "<view>{{ user.name }}</view>\n<view>{{ user.name }}</view>\n<view>{{ user.nmae }}</view>\n";
    let (temp, case) = setup(
        wxml,
        &valid_hover("{{ user.name }}", "user.name"),
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    let error = validate_fixture_case(&case, temp.path()).expect_err("duplicate marker");
    assert!(error.contains("exactly once"), "{error}");
}

#[test]
fn rejects_empty_markers() {
    let (temp, case) = setup(
        VALID_WXML,
        &valid_hover("", ""),
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    let error = validate_fixture_case(&case, temp.path()).expect_err("empty marker");
    assert!(error.contains("must not be empty"), "{error}");
}

#[test]
fn rejects_path_escape() {
    let temp = TempFixture::new();
    temp.write("outside.wxml", "<view>{{ user.name }}</view>\n");
    temp.write(
        "case/expected/diagnostics.json",
        &valid_diagnostics("{{ user.nmae }}", "nmae").to_string(),
    );
    temp.write(
        "case/expected/hover.json",
        &json!({
            "queries": [{
                "file": "../outside.wxml",
                "marker": "{{ user.name }}",
                "target": "user.name",
                "expect": { "type": "string", "typeKind": "string", "documentation": "doc" }
            }]
        })
        .to_string(),
    );
    let case = fixture_case(&json!(["{{ user.name }}"]), &json!(["{{ user.nmae }}"]));
    let error = validate_fixture_case(&case, temp.path()).expect_err("escape");
    assert!(error.contains("must not contain `..`"), "{error}");
}

#[test]
fn rejects_escaping_expected_path() {
    let temp = TempFixture::new();
    temp.write("outside.json", &valid_hover("{{ a }}", "a").to_string());
    temp.write("case/pages/index/index.wxml", VALID_WXML);
    temp.write(
        "case/expected/diagnostics.json",
        &valid_diagnostics("{{ user.nmae }}", "nmae").to_string(),
    );
    let mut case = fixture_case(&json!(["{{ user.name }}"]), &json!(["{{ user.nmae }}"]));
    case["expected"]["hover"] = json!("../outside.json");
    let error = validate_fixture_case(&case, temp.path()).expect_err("escape");
    assert!(error.contains("must not contain `..`"), "{error}");
}

#[test]
fn rejects_missing_documentation() {
    let hover = json!({
        "queries": [{
            "file": "pages/index/index.wxml",
            "marker": "{{ user.name }}",
            "target": "user.name",
            "expect": { "type": "string", "typeKind": "string" }
        }]
    });
    let (temp, case) = setup(
        VALID_WXML,
        &hover,
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    let error = validate_fixture_case(&case, temp.path()).expect_err("missing documentation");
    assert!(error.contains("documentation"), "{error}");
}

#[test]
fn rejects_wrong_position() {
    let hover = json!({
        "queries": [{
            "file": "pages/index/index.wxml",
            "marker": "{{ user.name }}",
            "target": "user.name",
            "position": { "line": 0, "character": 0 },
            "expect": { "type": "string", "typeKind": "string", "documentation": "doc" }
        }]
    });
    let (temp, case) = setup(
        VALID_WXML,
        &hover,
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    let error = validate_fixture_case(&case, temp.path()).expect_err("wrong position");
    assert!(error.contains("computed"), "{error}");
}

#[test]
fn rejects_diagnostics_without_negative_markers() {
    let (temp, case) = setup(
        VALID_WXML,
        &valid_hover("{{ user.name }}", "user.name"),
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    let mut case = case;
    case["negative"] = json!([]);
    let error = validate_fixture_case(&case, temp.path()).expect_err("missing negative");
    assert!(error.contains("negative markers"), "{error}");
}

#[test]
fn rejects_clean_cases_with_negative_markers() {
    let temp = TempFixture::new();
    temp.write("case/pages/index/index.wxml", VALID_WXML);
    temp.write(
        "case/expected/hover.json",
        &valid_hover("{{ user.name }}", "user.name").to_string(),
    );
    temp.write("case/expected/diagnostics.json", r#"{"diagnostics":[]}"#);
    let mut case = fixture_case(&json!(["{{ user.name }}"]), &json!(["{{ user.nmae }}"]));
    case["expectClean"] = json!(true);
    let error = validate_fixture_case(&case, temp.path()).expect_err("clean with negative");
    assert!(error.contains("negative markers"), "{error}");
}

#[test]
fn accepts_degraded_empty_diagnostics() {
    let temp = TempFixture::new();
    temp.write("case/pages/index/index.wxml", VALID_WXML);
    temp.write(
        "case/expected/hover.json",
        &valid_hover("{{ user.name }}", "user.name").to_string(),
    );
    temp.write(
        "case/expected/diagnostics.json",
        r#"{"status":"unsupported","diagnostics":[]}"#,
    );
    let case = fixture_case(&json!(["{{ user.name }}"]), &json!([]));
    validate_fixture_case(&case, temp.path()).expect("degraded case is valid");
}

#[test]
fn rejects_runtime_cases_without_steps() {
    let case = json!({
        "id": "runtime",
        "kind": "runtime",
        "acs": ["AC-009"],
        "scenario": "a scenario that describes the runtime behaviour",
        "steps": []
    });
    let error = validate_runtime_case(&case).expect_err("missing steps");
    assert!(error.contains("two steps"), "{error}");
}

#[test]
fn rejects_runtime_cases_with_todo_steps() {
    let case = json!({
        "id": "runtime",
        "kind": "runtime",
        "acs": ["AC-009"],
        "scenario": "a scenario that describes the runtime behaviour",
        "steps": [
            { "action": "TODO: open", "expect": "something" },
            { "action": "edit", "expect": "something" }
        ]
    });
    let error = validate_runtime_case(&case).expect_err("todo step");
    assert!(error.contains("TODO"), "{error}");
}

#[test]
fn rejects_manifest_version_one() {
    let manifest = json!({ "version": 1, "cases": [] });
    let error = validate_manifest(&manifest, Path::new("/nonexistent")).expect_err("wrong version");
    assert!(error.contains("version 2"), "{error}");
}

#[test]
fn rejects_escaping_root() {
    let temp = TempFixture::new();
    fs::create_dir_all(temp.path().join("fixtures")).expect("create fixtures root");
    let case = json!({
        "id": "case",
        "kind": "fixture",
        "acs": ["AC-001"],
        "root": "../outside",
        "positive": [],
        "negative": [],
        "expected": {
            "hover": "expected/hover.json",
            "diagnostics": "expected/diagnostics.json"
        }
    });
    let error =
        validate_fixture_case(&case, &temp.path().join("fixtures")).expect_err("root escapes");
    assert!(error.contains("must not contain `..`"), "{error}");
}

#[test]
fn rejects_absolute_root() {
    let temp = TempFixture::new();
    fs::create_dir_all(temp.path().join("fixtures")).expect("create fixtures root");
    let absolute = temp.path().join("case").to_string_lossy().into_owned();
    let case = json!({
        "id": "case",
        "kind": "fixture",
        "acs": ["AC-001"],
        "root": absolute,
        "positive": [],
        "negative": [],
        "expected": {
            "hover": "expected/hover.json",
            "diagnostics": "expected/diagnostics.json"
        }
    });
    let error =
        validate_fixture_case(&case, &temp.path().join("fixtures")).expect_err("absolute root");
    assert!(error.contains("must be relative"), "{error}");
}

#[test]
fn accepts_expect_level_range_target() {
    let hover = json!({
        "queries": [{
            "file": "pages/index/index.wxml",
            "marker": "{{ user.name }}",
            "target": "user.name",
            "expect": {
                "type": "string",
                "typeKind": "string",
                "documentation": "doc",
                "rangeTarget": "user"
            }
        }]
    });
    let (temp, case) = setup(
        VALID_WXML,
        &hover,
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    validate_fixture_case(&case, temp.path()).expect("expect-level rangeTarget is valid");
}

#[test]
fn rejects_query_level_range_target() {
    let hover = json!({
        "queries": [{
            "file": "pages/index/index.wxml",
            "marker": "{{ user.name }}",
            "target": "user.name",
            "rangeTarget": "user",
            "expect": { "type": "string", "typeKind": "string", "documentation": "doc" }
        }]
    });
    let (temp, case) = setup(
        VALID_WXML,
        &hover,
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    let error = validate_fixture_case(&case, temp.path()).expect_err("query-level rangeTarget");
    assert!(error.contains("unknown field `rangeTarget`"), "{error}");
}

#[test]
fn rejects_ac012_without_definition() {
    let case = json!({
        "id": "case",
        "acs": ["AC-012"],
        "coverage": { "edits": "deferred-to-C1" },
        "expected": {
            "hover": "expected/hover.json",
            "diagnostics": "expected/diagnostics.json"
        }
    });
    let object = case.as_object().expect("case object");
    let error = validate_ac012_case(object, "case").expect_err("missing definition");
    assert!(error.contains("definition"), "{error}");

    let mut with_definition = case.clone();
    with_definition["expected"]["definition"] = json!("expected/definition.json");
    let object = with_definition.as_object().expect("case object");
    validate_ac012_case(object, "case").expect("definition is present");
}

const CROSS_FILE_WXML: &str = "<view>{{ user.name }}</view>\n<view>{{ user.nmae }}</view>\n";

fn cross_file_setup() -> (TempFixture, Value) {
    let temp = TempFixture::new();
    temp.write("case/pages/a.wxml", CROSS_FILE_WXML);
    temp.write("case/pages/b.wxml", CROSS_FILE_WXML);
    let hover = json!({
        "queries": [
            {
                "file": "pages/a.wxml",
                "marker": "{{ user.name }}",
                "target": "user.name",
                "expect": { "type": "string", "typeKind": "string", "documentation": "doc" }
            },
            {
                "file": "pages/b.wxml",
                "marker": "{{ user.name }}",
                "target": "user.name",
                "expect": { "type": "string", "typeKind": "string", "documentation": "doc" }
            }
        ]
    });
    let diagnostics = json!({
        "diagnostics": [
            {
                "file": "pages/a.wxml",
                "marker": "{{ user.nmae }}",
                "target": "nmae",
                "severity": "error",
                "messageContains": "nmae"
            },
            {
                "file": "pages/b.wxml",
                "marker": "{{ user.nmae }}",
                "target": "nmae",
                "severity": "error",
                "messageContains": "nmae"
            }
        ]
    });
    temp.write("case/expected/hover.json", &hover.to_string());
    temp.write("case/expected/diagnostics.json", &diagnostics.to_string());
    let case = fixture_case(
        &json!(["{{ user.name }}", "{{ user.name }}"]),
        &json!(["{{ user.nmae }}", "{{ user.nmae }}"]),
    );
    (temp, case)
}

#[test]
fn accepts_the_same_marker_in_two_files() {
    let (temp, case) = cross_file_setup();
    validate_fixture_case(&case, temp.path()).expect("cross-file duplicates are counted");
}

#[test]
fn rejects_missing_cross_file_hover_entry() {
    let (temp, case) = cross_file_setup();
    let mut hover = read_json(&temp.path().join("case/expected/hover.json")).expect("hover parses");
    hover["queries"].as_array_mut().expect("queries").pop();
    temp.write("case/expected/hover.json", &hover.to_string());
    let error = validate_fixture_case(&case, temp.path()).expect_err("missing CRLF hover");
    assert!(error.contains("positive markers"), "{error}");
}

#[test]
fn rejects_missing_cross_file_diagnostic_entry() {
    let (temp, case) = cross_file_setup();
    let mut diagnostics =
        read_json(&temp.path().join("case/expected/diagnostics.json")).expect("diagnostics parse");
    diagnostics["diagnostics"]
        .as_array_mut()
        .expect("diagnostics")
        .pop();
    temp.write("case/expected/diagnostics.json", &diagnostics.to_string());
    let error = validate_fixture_case(&case, temp.path()).expect_err("missing CRLF diagnostic");
    assert!(error.contains("negative markers"), "{error}");
}

#[test]
fn rejects_missing_required_position() {
    let (temp, mut case) = setup(
        VALID_WXML,
        &valid_hover("{{ user.name }}", "user.name"),
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    case["requirePositions"] = json!(true);
    let error = validate_fixture_case(&case, temp.path()).expect_err("missing position");
    assert!(error.contains("position"), "{error}");
}

#[test]
fn accepts_required_position_when_correct() {
    let hover = json!({
        "queries": [{
            "file": "pages/index/index.wxml",
            "marker": "{{ user.name }}",
            "target": "user.name",
            "position": { "line": 0, "character": 9 },
            "expect": { "type": "string", "typeKind": "string", "documentation": "doc" }
        }]
    });
    let (temp, mut case) = setup(
        VALID_WXML,
        &hover,
        &valid_diagnostics("{{ user.nmae }}", "nmae"),
    );
    case["requirePositions"] = json!(true);
    validate_fixture_case(&case, temp.path()).expect("position is present and correct");
}

#[test]
fn rejects_diagnostic_entry_status() {
    let diagnostics = json!({
        "diagnostics": [{
            "file": "pages/index/index.wxml",
            "marker": "{{ user.nmae }}",
            "target": "nmae",
            "severity": "error",
            "messageContains": "nmae",
            "status": "supported"
        }]
    });
    let (temp, case) = setup(
        VALID_WXML,
        &valid_hover("{{ user.name }}", "user.name"),
        &diagnostics,
    );
    let error = validate_fixture_case(&case, temp.path()).expect_err("diagnostic status");
    assert!(error.contains("unknown field `status`"), "{error}");
}

fn unicode_manifest_case() -> Value {
    let manifest =
        read_json(&manifest_path()).unwrap_or_else(|error| panic!("manifest must parse: {error}"));
    manifest["cases"]
        .as_array()
        .unwrap_or_else(|| panic!("manifest cases must be an array"))
        .iter()
        .find(|case| case["id"].as_str() == Some("unicode"))
        .unwrap_or_else(|| panic!("unicode case must exist"))
        .clone()
}

fn copy_unicode_case(hover: &Value, diagnostics: &Value) -> (TempFixture, Value) {
    let temp = TempFixture::new();
    let source = fixtures_dir().join("unicode");
    for file in ["pages/index/index.wxml", "pages/index/index-crlf.wxml"] {
        let contents = fs::read_to_string(source.join(file))
            .unwrap_or_else(|error| panic!("failed to read unicode fixture: {error}"));
        temp.write(&format!("unicode/{file}"), &contents);
    }
    temp.write("unicode/expected/hover.json", &hover.to_string());
    temp.write(
        "unicode/expected/diagnostics.json",
        &diagnostics.to_string(),
    );
    (temp, unicode_manifest_case())
}

#[test]
fn rejects_missing_unicode_crlf_hover() {
    let mut hover = read_json(&fixtures_dir().join("unicode/expected/hover.json"))
        .expect("unicode hover parses");
    let queries = hover["queries"].as_array_mut().expect("queries");
    let index = queries
        .iter()
        .position(|query| query["file"].as_str() == Some("pages/index/index-crlf.wxml"))
        .expect("CRLF hover entry");
    queries.remove(index);
    let diagnostics = read_json(&fixtures_dir().join("unicode/expected/diagnostics.json"))
        .expect("unicode diagnostics parse");
    let (temp, case) = copy_unicode_case(&hover, &diagnostics);
    let error = validate_fixture_case(&case, temp.path()).expect_err("missing CRLF hover");
    assert!(error.contains("positive markers"), "{error}");
}

#[test]
fn rejects_missing_unicode_crlf_diagnostic() {
    let hover = read_json(&fixtures_dir().join("unicode/expected/hover.json"))
        .expect("unicode hover parses");
    let mut diagnostics = read_json(&fixtures_dir().join("unicode/expected/diagnostics.json"))
        .expect("unicode diagnostics parse");
    let entries = diagnostics["diagnostics"]
        .as_array_mut()
        .expect("diagnostics");
    let index = entries
        .iter()
        .position(|entry| entry["file"].as_str() == Some("pages/index/index-crlf.wxml"))
        .expect("CRLF diagnostic entry");
    entries.remove(index);
    let (temp, case) = copy_unicode_case(&hover, &diagnostics);
    let error = validate_fixture_case(&case, temp.path()).expect_err("missing CRLF diagnostic");
    assert!(error.contains("negative markers"), "{error}");
}
