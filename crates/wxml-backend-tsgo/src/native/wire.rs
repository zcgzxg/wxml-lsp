//! Wire DTOs and parameter builders for the native API.
//!
//! Field names mirror the pinned engine's generated protocol types:
//! `openFiles`/`closeFiles`/`openProjects`/`closeProjects`, nested
//! `changes` for `updateSnapshot`, camelCase query responses and
//! `{ "uri": ... }` document identifiers. Unknown response fields are ignored;
//! missing required fields produce a typed shape error in the session.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::native::NativeError;

/// Encodes an absolute Unix path as a `file://` URI.
///
/// Characters outside the URI path set are percent-encoded as UTF-8 bytes.
/// Percent-encoding is deliberately minimal and lossless; the engine decodes it
/// back to the same path.
#[must_use]
pub fn file_uri(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut uri = String::with_capacity(path.len() + 8);
    uri.push_str("file://");
    for &byte in path.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            if let Some(&high) = HEX.get(usize::from(byte >> 4)) {
                uri.push(char::from(high));
            }
            if let Some(&low) = HEX.get(usize::from(byte & 0x0f)) {
                uri.push(char::from(low));
            }
        }
    }
    uri
}

/// One `{ uri }` document identifier.
#[must_use]
pub fn document_identifier(path: &str) -> Value {
    json!({ "uri": file_uri(path) })
}

/// Virtual filesystem supplied to `createSnapshot`/`updateSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFileSystem {
    files: BTreeMap<String, String>,
}

impl SnapshotFileSystem {
    /// Creates a `kind: "full"` filesystem from absolute paths and texts.
    #[must_use]
    pub const fn full(files: BTreeMap<String, String>) -> Self {
        Self { files }
    }

    /// Returns the supplied files.
    #[must_use]
    pub const fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }

    /// Returns the `kind: "full"` value consumed by the engine.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "kind": "full",
            "files": &self.files,
        })
    }
}

/// Snapshot changes shared by create and update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotChanges {
    open_files: Vec<String>,
    close_files: Vec<String>,
    open_projects: Vec<String>,
    close_projects: Vec<String>,
    file_system: Option<SnapshotFileSystem>,
    prepare_auto_imports: Option<String>,
}

impl SnapshotChanges {
    /// Creates empty changes.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            open_files: Vec::new(),
            close_files: Vec::new(),
            open_projects: Vec::new(),
            close_projects: Vec::new(),
            file_system: None,
            prepare_auto_imports: None,
        }
    }

    /// Adds an open file.
    #[must_use]
    pub fn open_file(mut self, path: impl Into<String>) -> Self {
        self.open_files.push(path.into());
        self
    }

    /// Adds a closed file.
    #[must_use]
    pub fn close_file(mut self, path: impl Into<String>) -> Self {
        self.close_files.push(path.into());
        self
    }

    /// Adds an open project (tsconfig path).
    #[must_use]
    pub fn open_project(mut self, path: impl Into<String>) -> Self {
        self.open_projects.push(path.into());
        self
    }

    /// Adds a closed project (tsconfig path).
    #[must_use]
    pub fn close_project(mut self, path: impl Into<String>) -> Self {
        self.close_projects.push(path.into());
        self
    }

    /// Sets the virtual filesystem.
    #[must_use]
    pub fn with_file_system(mut self, file_system: SnapshotFileSystem) -> Self {
        self.file_system = Some(file_system);
        self
    }

    /// Asks the engine to make auto-import indexes ready for one absolute file.
    ///
    /// `prepareAutoImports` is a top-level `CreateSnapshotParams` member, so
    /// [`create_snapshot_params`] emits it at the top level and
    /// [`update_snapshot_params`] emits it under `changes`. This is the only
    /// API-level way to enable auto-import completion: an unprepared snapshot
    /// rejects `includeSymbol:true` with an engine error.
    #[must_use]
    pub fn prepare_auto_imports(mut self, path: impl Into<String>) -> Self {
        self.prepare_auto_imports = Some(path.into());
        self
    }

    /// Returns the auto-import preparation path, when set.
    #[must_use]
    pub fn prepare_auto_imports_path(&self) -> Option<&str> {
        self.prepare_auto_imports.as_deref()
    }

    /// Returns the open files.
    #[must_use]
    pub fn open_files(&self) -> &[String] {
        &self.open_files
    }

    /// Returns the closed files.
    #[must_use]
    pub fn close_files(&self) -> &[String] {
        &self.close_files
    }

    /// Builds the engine `CreateSnapshotParams`/`SnapshotRequestChangesParams`
    /// value. Empty identifier lists are omitted.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        insert_identifiers(&mut object, "openFiles", &self.open_files);
        insert_identifiers(&mut object, "closeFiles", &self.close_files);
        insert_identifiers(&mut object, "openProjects", &self.open_projects);
        insert_identifiers(&mut object, "closeProjects", &self.close_projects);
        if let Some(file_system) = &self.file_system {
            object.insert("fileSystem".to_owned(), file_system.to_value());
        }
        if let Some(path) = &self.prepare_auto_imports {
            object.insert("prepareAutoImports".to_owned(), document_identifier(path));
        }
        Value::Object(object)
    }
}

/// Builds a `createSnapshot` params value.
#[must_use]
pub fn create_snapshot_params(changes: &SnapshotChanges) -> Value {
    changes.to_value()
}

/// Builds an `updateSnapshot` params value with nested `changes`.
#[must_use]
pub fn update_snapshot_params(snapshot: u64, changes: &SnapshotChanges) -> Value {
    json!({
        "snapshot": snapshot,
        "changes": changes.to_value(),
    })
}

/// Builds `{snapshot, project}` params.
#[must_use]
pub fn project_params(snapshot: u64, project: &str) -> Value {
    json!({ "snapshot": snapshot, "project": project })
}

/// Builds `{snapshot, project, file, position?}` params.
#[must_use]
pub fn file_params(snapshot: u64, project: &str, path: &str, position: Option<u32>) -> Value {
    let mut object = Map::new();
    object.insert("snapshot".to_owned(), json!(snapshot));
    object.insert("project".to_owned(), json!(project));
    object.insert("file".to_owned(), document_identifier(path));
    if let Some(position) = position {
        object.insert("position".to_owned(), json!(position));
    }
    Value::Object(object)
}

/// Builds `{snapshot, project, files:[{uri}]}` params for diagnostics.
#[must_use]
pub fn files_params(snapshot: u64, project: &str, paths: &[&str]) -> Value {
    json!({
        "snapshot": snapshot,
        "project": project,
        "files": paths.iter().map(|path| document_identifier(path)).collect::<Vec<_>>(),
    })
}

/// Builds `{snapshot, project, type}` params.
#[must_use]
pub fn type_params(snapshot: u64, project: &str, type_id: u64) -> Value {
    json!({ "snapshot": snapshot, "project": project, "type": type_id })
}

/// Builds `{snapshot, project, symbol}` params.
#[must_use]
pub fn symbol_params(snapshot: u64, project: &str, reference: &Value) -> Value {
    json!({ "snapshot": snapshot, "project": project, "symbol": reference })
}

/// Builds `{snapshot, project, node, position}` params.
#[must_use]
pub fn node_params(snapshot: u64, project: &str, node: &str, position: u32) -> Value {
    json!({
        "snapshot": snapshot,
        "project": project,
        "node": node,
        "position": position,
    })
}

/// Builds completion params with an explicit `includeSymbol` flag.
///
/// The pinned engine exposes no per-entry auto-import origin or import edit;
/// `includeSymbol:true` only attaches symbol evidence and **requires** a
/// snapshot created/updated with `prepareAutoImports`. Sending it on an
/// unprepared snapshot is an engine error, so callers must prepare first.
#[must_use]
pub fn completion_params(
    snapshot: u64,
    project: &str,
    path: &str,
    position: u32,
    include_symbol: bool,
    trigger: Option<&str>,
) -> Value {
    let mut object = Map::new();
    object.insert("snapshot".to_owned(), json!(snapshot));
    object.insert("project".to_owned(), json!(project));
    object.insert("file".to_owned(), document_identifier(path));
    object.insert("position".to_owned(), json!(position));
    object.insert("includeSymbol".to_owned(), json!(include_symbol));
    if let Some(trigger) = trigger {
        object.insert("triggerCharacter".to_owned(), json!(trigger));
    }
    Value::Object(object)
}

/// Builds `{snapshot}` params for release.
#[must_use]
pub fn release_params(snapshot: u64) -> Value {
    json!({ "snapshot": snapshot })
}

/// Builds `{snapshot, project, file}` params for `getSourceFile`.
#[must_use]
pub fn source_file_params(snapshot: u64, project: &str, path: &str) -> Value {
    json!({
        "snapshot": snapshot,
        "project": project,
        "file": document_identifier(path),
    })
}

fn insert_identifiers(object: &mut Map<String, Value>, key: &str, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    object.insert(
        key.to_owned(),
        Value::Array(paths.iter().map(|path| document_identifier(path)).collect()),
    );
}

/// Parses one response value into a typed DTO.
///
/// # Errors
///
/// Returns a bounded [`NativeError::Protocol`] when the shape is wrong. The
/// serde detail is intentionally not surfaced to user-visible errors.
pub fn parse_response<T: DeserializeOwned>(value: Value) -> Result<T, NativeError> {
    serde_json::from_value(value)
        .map_err(|_| NativeError::Protocol("native response shape mismatch".to_owned()))
}

/// Parses a nullable response value into `Option<T>`.
///
/// # Errors
///
/// Propagates [`parse_response`] for non-null values.
pub fn parse_optional<T: DeserializeOwned>(value: Value) -> Result<Option<T>, NativeError> {
    if value.is_null() {
        Ok(None)
    } else {
        parse_response(value).map(Some)
    }
}

/// One native diagnostic exactly as returned by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawDiagnostic {
    /// Numeric diagnostic code.
    pub code: i64,
    /// Numeric category (`0` warning, `1` error, `2` suggestion, `3` message).
    pub category: i64,
    /// Engine file the diagnostic belongs to, when the response carries one.
    ///
    /// `getSemanticDiagnostics` is requested for one file, but the pinned
    /// response shape allows a different owner (for example an external
    /// declaration). Callers must not assume every entry belongs to the
    /// queried file when this field is present.
    #[serde(default)]
    pub file_name: Option<String>,
    /// Engine UTF-16 full-start offset, when present.
    #[serde(default)]
    pub pos: Option<i64>,
    /// Engine UTF-16 exclusive end offset, when present.
    #[serde(default)]
    pub end: Option<i64>,
    /// Zero-based line/UTF-16 column start.
    #[serde(default)]
    pub start_position: Option<RawPosition>,
    /// Zero-based line/UTF-16 column end.
    #[serde(default)]
    pub end_position: Option<RawPosition>,
    /// Diagnostic message text.
    pub text: String,
}

/// One zero-based line/column pair from the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct RawPosition {
    /// Zero-based line.
    pub line: i64,
    /// Zero-based UTF-16 column.
    pub character: i64,
}

/// One native type object.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawType {
    /// Opaque engine type id.
    pub id: u64,
    /// Type flags.
    #[serde(default)]
    pub flags: u64,
    /// Intrinsic name (`any`, `error`, ...) when the engine reports one.
    #[serde(default)]
    pub intrinsic_name: Option<String>,
}

/// One native symbol response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSymbol {
    /// Symbol name.
    pub name: String,
    /// Symbol flags.
    #[serde(default)]
    pub flags: u64,
    /// Node handles for declarations.
    #[serde(default)]
    pub declarations: Vec<String>,
    /// Node handle for the value declaration.
    #[serde(default)]
    pub value_declaration: Option<String>,
    /// Content-addressed reference descriptor.
    #[serde(default)]
    pub reference: Option<RawSymbolReference>,
}

/// `SymbolOwnerKind.File`: the reference carries a full source-file descriptor.
pub const SYMBOL_OWNER_FILE: u64 = 0;

/// `SymbolOwnerKind.Snapshot`: the engine interns the symbol in the response
/// snapshot, so the reference carries `snapshot` and `project` instead of a
/// source-file descriptor.
pub const SYMBOL_OWNER_SNAPSHOT: u64 = 1;

/// Symbol reference accepted by documentation and symbol queries.
///
/// The pinned engine emits two owner forms behind the same field:
///
/// - file-owned (`kind = 0`): a full [`RawSourceFileDescriptor`] with content
///   hash, parse-options key and node id;
/// - snapshot-owned (`kind = 1`): `snapshot` + `project` with no file
///   descriptor. The engine interns the symbol in that snapshot, and a
///   cross-snapshot reference is rejected by the engine itself.
///
/// Both forms are serialized back unchanged; absent optional fields are
/// omitted so the engine receives the same shape it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSymbolReference {
    /// Engine symbol id.
    pub id: u64,
    /// Owner kind; see [`SYMBOL_OWNER_FILE`] and [`SYMBOL_OWNER_SNAPSHOT`].
    #[serde(default)]
    pub kind: u64,
    /// Source-file descriptor for a file-owned reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<RawSourceFileDescriptor>,
    /// Snapshot id for a snapshot-owned reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<u64>,
    /// Project id for a snapshot-owned reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl RawSymbolReference {
    /// Returns the file descriptor of a file-owned reference.
    #[must_use]
    pub const fn file_descriptor(&self) -> Option<&RawSourceFileDescriptor> {
        self.file.as_ref()
    }

    /// Returns whether this is a complete file-owned reference.
    #[must_use]
    pub const fn is_file_owned(&self) -> bool {
        self.kind == SYMBOL_OWNER_FILE && self.file.is_some()
    }

    /// Returns whether this is a complete snapshot-owned reference.
    #[must_use]
    pub const fn is_snapshot_owned(&self) -> bool {
        self.kind == SYMBOL_OWNER_SNAPSHOT
            && self.file.is_none()
            && self.snapshot.is_some()
            && self.project.is_some()
    }
}

/// Source-file descriptor embedded in a symbol reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSourceFileDescriptor {
    /// Engine file name.
    pub file_name: String,
    /// Normalized path.
    #[serde(default)]
    pub path: String,
    /// Content hash.
    #[serde(default)]
    pub content_hash: String,
    /// Parse-options key.
    #[serde(default)]
    pub parse_options_key: String,
    /// Script kind.
    #[serde(default)]
    pub script_kind: i64,
    /// Node id.
    #[serde(default)]
    pub node_id: String,
}

/// One `JSDoc` tag.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RawJsDocTag {
    /// Tag name without `@`.
    pub name: String,
    /// Optional tag text.
    #[serde(default)]
    pub text: Option<String>,
}

/// One referenced-symbol group.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawReferencedSymbol {
    /// Definition node handle.
    pub definition: String,
    /// Reference node handles.
    #[serde(default)]
    pub references: Vec<String>,
    /// Symbol summary.
    #[serde(default)]
    pub symbol: Option<RawSymbol>,
}

/// Optional label details carried by one completion entry.
///
/// The pinned 7.1.0-beta artifact emits only `name`, `kind` and `sortText`
/// for completion entries. These fields are tolerated if a future pinned
/// artifact adds them: `detail` is mapped as signature text and `description`
/// is never treated as documentation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCompletionLabelDetails {
    /// Short label detail such as a signature.
    #[serde(default)]
    pub detail: Option<String>,
    /// Additional description (auto-import source metadata in the LSP path).
    #[serde(default)]
    pub description: Option<String>,
}

/// One completion entry with the fields the pinned v1 response exposes.
///
/// The 7.1.0-beta artifact emits `name`, `kind`, `sortText` and, when
/// `includeSymbol:true` was requested on a prepared snapshot, `symbol`. The
/// optional `insertText` / `filterText` / `detail` / `labelDetails` members are
/// tolerated when present. There is deliberately no `origin`, `source`,
/// `data`, `documentation` or `textEdit` member: the pinned response has none,
/// so an auto-import candidate cannot be identified or edited from this DTO.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCompletionEntry {
    /// Display name.
    pub name: String,
    /// Native LSP `CompletionItemKind` number; `0` means the field was absent.
    #[serde(default)]
    pub kind: i64,
    /// Native sort text.
    #[serde(default)]
    pub sort_text: String,
    /// Native insertion text; may use snippet syntax.
    #[serde(default)]
    pub insert_text: Option<String>,
    /// Native filter text.
    #[serde(default)]
    pub filter_text: Option<String>,
    /// Native short detail (type/signature text) when a future artifact
    /// supplies it; 7.1.0-beta emits none.
    #[serde(default)]
    pub detail: Option<String>,
    /// Optional label details tolerated from a future artifact.
    #[serde(default)]
    pub label_details: Option<RawCompletionLabelDetails>,
    /// Symbol evidence; only populated when the request set
    /// `includeSymbol:true` on a snapshot prepared for auto-imports.
    ///
    /// This is not an auto-import origin: ordinary imported members carry a
    /// symbol too. Handles and file names inside it are engine internals and
    /// must never be published.
    #[serde(default)]
    pub symbol: Option<RawSymbol>,
}

/// One completion list.
///
/// The `entries` array is required: a response that omits it is shape drift
/// and must surface as a typed shape error instead of failing open as an empty
/// completion list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCompletionList {
    /// Whether the engine considers the list incomplete.
    ///
    /// Absent follows the LSP default (`false`). Non-empty results still stay
    /// `Partial` for the stable documentation/auto-import gaps, so an absent
    /// flag cannot publish a complete member list.
    #[serde(default)]
    pub is_incomplete: bool,
    /// Entries in native order.
    pub entries: Vec<RawCompletionEntry>,
}

/// Binary `getSourceFile` response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RawSourceFile {
    /// Base64-encoded v9 AST payload.
    pub data: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_are_lossless_and_percent_encoded() {
        assert_eq!(file_uri("/work/main.ts"), "file:///work/main.ts");
        assert_eq!(file_uri("/work/a b.ts"), "file:///work/a%20b.ts");
        assert_eq!(file_uri("/work/é.ts"), "file:///work/%C3%A9.ts");
        assert_eq!(file_uri("/work/#?.ts"), "file:///work/%23%3F.ts");
    }

    #[test]
    fn snapshot_params_match_the_engine_field_names() {
        let changes = SnapshotChanges::new()
            .open_file("/work/main.ts")
            .close_file("/work/old.ts")
            .open_project("/work/tsconfig.json")
            .with_file_system(SnapshotFileSystem::full(BTreeMap::from([(
                "/work/main.ts".to_owned(),
                "const x = 1;".to_owned(),
            )])));

        let value = changes.to_value();
        assert_eq!(value["openFiles"], json!([{"uri": "file:///work/main.ts"}]));
        assert_eq!(value["closeFiles"], json!([{"uri": "file:///work/old.ts"}]));
        assert_eq!(
            value["openProjects"],
            json!([{"uri": "file:///work/tsconfig.json"}])
        );
        assert!(value.get("closeProjects").is_none());
        assert_eq!(value["fileSystem"]["kind"], "full");
        assert_eq!(
            value["fileSystem"]["files"]["/work/main.ts"],
            "const x = 1;"
        );

        let update = update_snapshot_params(7, &changes);
        assert_eq!(update["snapshot"], 7);
        assert_eq!(
            update["changes"]["openFiles"][0]["uri"],
            "file:///work/main.ts"
        );
    }

    #[test]
    fn prepare_auto_imports_is_a_document_identifier_on_create_and_update() {
        let changes = SnapshotChanges::new().prepare_auto_imports("/work/main.ts");
        assert_eq!(changes.prepare_auto_imports_path(), Some("/work/main.ts"));

        let create = create_snapshot_params(&changes);
        assert_eq!(
            create["prepareAutoImports"],
            json!({"uri": "file:///work/main.ts"})
        );
        assert_eq!(create["prepareAutoImports"]["uri"], "file:///work/main.ts");

        let update = update_snapshot_params(9, &changes);
        assert_eq!(
            update["changes"]["prepareAutoImports"],
            json!({"uri": "file:///work/main.ts"})
        );

        // An unset path must not be sent as `null` or an empty object.
        let plain = create_snapshot_params(&SnapshotChanges::new());
        assert!(plain.get("prepareAutoImports").is_none());
        assert!(SnapshotChanges::new().prepare_auto_imports_path().is_none());
    }

    #[test]
    fn query_params_use_integers_and_document_identifiers() {
        assert_eq!(
            file_params(1, "/dev/null/inferred", "/work/main.ts", Some(42)),
            json!({
                "snapshot": 1,
                "project": "/dev/null/inferred",
                "file": {"uri": "file:///work/main.ts"},
                "position": 42,
            })
        );
        assert_eq!(
            files_params(1, "p", &["/work/main.ts"]),
            json!({
                "snapshot": 1,
                "project": "p",
                "files": [{"uri": "file:///work/main.ts"}],
            })
        );
        assert_eq!(
            completion_params(1, "p", "/work/main.ts", 3, false, Some(".")),
            json!({
                "snapshot": 1,
                "project": "p",
                "file": {"uri": "file:///work/main.ts"},
                "position": 3,
                "includeSymbol": false,
                "triggerCharacter": ".",
            })
        );
        assert_eq!(
            completion_params(1, "p", "/work/main.ts", 3, true, None)["includeSymbol"],
            json!(true)
        );
    }

    #[test]
    fn parses_evidence_shaped_responses() {
        let diagnostic: RawDiagnostic = parse_response(json!({
            "fileName": "/work/main.ts",
            "pos": 112,
            "end": 119,
            "startPosition": {"line": 3, "character": 2},
            "endPosition": {"line": 3, "character": 9},
            "code": 2339,
            "category": 1,
            "text": "Property 'missing' does not exist on type 'User'.",
            "sourceLines": [{"line": 3, "text": "u.missing;\n"}],
        }))
        .expect("diagnostic");
        assert_eq!(diagnostic.code, 2339);
        assert_eq!(diagnostic.file_name.as_deref(), Some("/work/main.ts"));
        assert_eq!(diagnostic.start_position.expect("start").line, 3);

        let unnamed: RawDiagnostic = parse_response(json!({
            "code": 1109,
            "category": 1,
            "text": "expected expression",
        }))
        .expect("diagnostic without fileName");
        assert_eq!(unnamed.file_name, None);

        let symbol: RawSymbol = parse_response(json!({
            "name": "u",
            "flags": 2,
            "declarations": ["15.262./work/main.ts"],
            "reference": {
                "id": 281_474_976_710_662_u64,
                "kind": 0,
                "file": {
                    "fileName": "/work/main.ts",
                    "path": "/work/main.ts",
                    "contentHash": "ed07a28c52d39dfef9c12945c739f284",
                    "parseOptionsKey": "0",
                    "scriptKind": 3,
                    "nodeId": "1",
                },
            },
        }))
        .expect("symbol");
        assert_eq!(symbol.name, "u");
        let reference = symbol.reference.expect("reference");
        assert_eq!(
            reference.file.as_ref().expect("file").content_hash,
            "ed07a28c52d39dfef9c12945c739f284"
        );
        let encoded = serde_json::to_value(&reference).expect("serialize reference");
        assert_eq!(encoded["file"]["nodeId"], "1");

        let completions: RawCompletionList = parse_response(json!({
            "isIncomplete": false,
            "entries": [
                {"name": "name", "kind": 5, "sortText": "11"},
                {"name": "age", "kind": 5, "sortText": "11"},
            ],
        }))
        .expect("completions");
        assert_eq!(completions.entries.len(), 2);
        assert_eq!(completions.entries[0].sort_text, "11");
    }

    #[test]
    fn snapshot_owned_symbol_reference_deserializes_without_a_file() {
        let symbol: RawSymbol = parse_response(json!({
            "name": "name",
            "flags": 33_554_436,
            "checkFlags": 2306,
            "declarations": ["38.304./work/demo/pages/index/index.ts"],
            "valueDeclaration": "38.304./work/demo/pages/index/index.ts",
            "reference": {
                "id": 281_474_976_711_565_u64,
                "kind": 1,
                "project": "/work/demo/tsconfig.json",
                "snapshot": 2,
            },
        }))
        .expect("snapshot-owned symbol");
        let reference = symbol.reference.expect("reference");
        assert!(reference.is_snapshot_owned());
        assert!(!reference.is_file_owned());
        assert_eq!(reference.snapshot, Some(2));
        assert_eq!(
            reference.project.as_deref(),
            Some("/work/demo/tsconfig.json")
        );

        let encoded = serde_json::to_value(&reference).expect("serialize reference");
        assert!(encoded.get("file").is_none());
        assert_eq!(encoded["project"], "/work/demo/tsconfig.json");
        assert_eq!(encoded["snapshot"], 2);
    }

    #[test]
    fn parses_optional_completion_detail_fields() {
        let completions: RawCompletionList = parse_response(json!({
            "isIncomplete": false,
            "entries": [
                {
                    "name": "name",
                    "kind": 5,
                    "sortText": "11",
                    "detail": "name: string",
                    "labelDetails": {"detail": "(value: string): void"},
                },
                {"name": "age", "kind": 10, "sortText": "12"},
            ],
        }))
        .expect("completions");
        assert_eq!(
            completions.entries[0].detail.as_deref(),
            Some("name: string")
        );
        assert_eq!(
            completions.entries[0]
                .label_details
                .as_ref()
                .and_then(|details| details.detail.as_deref()),
            Some("(value: string): void")
        );
        assert!(completions.entries[1].detail.is_none());
        assert!(completions.entries[1].label_details.is_none());
    }

    #[test]
    fn parses_completion_symbol_and_insert_text_evidence() {
        let completions: RawCompletionList = parse_response(json!({
            "isIncomplete": false,
            "entries": [
                {
                    "name": "User",
                    "kind": 8,
                    "sortText": "16",
                    "insertText": "User",
                    "filterText": "User",
                    "detail": "interface User",
                    "symbol": {
                        "name": "User",
                        "flags": 5_242_880,
                        "declarations": ["22.262./work/model.ts"],
                        "reference": {
                            "id": 281_474_976_711_001_u64,
                            "kind": 1,
                            "project": "/work/tsconfig.json",
                            "snapshot": 3,
                        },
                    },
                },
            ],
        }))
        .expect("completions");
        let entry = &completions.entries[0];
        assert_eq!(entry.insert_text.as_deref(), Some("User"));
        assert_eq!(entry.filter_text.as_deref(), Some("User"));
        let symbol = entry.symbol.as_ref().expect("symbol evidence");
        assert_eq!(symbol.name, "User");
        assert_eq!(symbol.flags, 5_242_880);
        assert_eq!(symbol.declarations.len(), 1);
    }

    #[test]
    fn an_entry_without_symbol_fields_keeps_them_absent() {
        let completions: RawCompletionList = parse_response(json!({
            "isIncomplete": false,
            "entries": [{"name": "name", "kind": 5, "sortText": "11"}],
        }))
        .expect("completions");
        let entry = &completions.entries[0];
        assert!(entry.insert_text.is_none());
        assert!(entry.filter_text.is_none());
        assert!(entry.symbol.is_none());
    }

    #[test]
    fn a_completion_response_without_entries_is_a_shape_error() {
        let error = parse_response::<RawCompletionList>(json!({"isIncomplete": false}))
            .expect_err("absent entries must not fail open as an empty list");
        assert!(matches!(error, NativeError::Protocol(_)), "{error:?}");
    }
}
