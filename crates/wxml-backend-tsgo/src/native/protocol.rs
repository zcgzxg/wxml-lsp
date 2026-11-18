//! Typed native method names and the minimal P3 response DTOs.
//!
//! [`NativeMethod`] is the only way to name a wire method; there is
//! deliberately no `call(method, params)` escape hatch. The allow-list itself
//! is data in [`crate::config`], and a unit test keeps both in sync.
//!
//! The response DTOs cover exactly what the transport layer needs to prove a
//! session is usable: the `initialize` handshake, snapshot creation/update,
//! the snapshot file list, an opaque type handle and text results. They are
//! deliberately small: hover, symbol, diagnostic and completion semantics
//! belong to `P4` and must not be invented here. Unknown fields are ignored;
//! missing or mistyped required fields are typed [`ShapeError`]s.

use serde_json::{Map, Value};
use thiserror::Error;

use crate::native::rpc::RawPayload;

macro_rules! native_methods {
    ($($variant:ident => $wire:literal,)+) => {
        /// Every method permitted by the frozen F0 allow-list.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum NativeMethod {
            $(
                #[doc = concat!("The native `", $wire, "` method.")]
                $variant,
            )+
        }

        impl NativeMethod {
            /// All variants in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// Returns the exact wire name sent in `method`.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }
        }
    };
}

native_methods! {
    Initialize => "initialize",
    CreateSnapshot => "createSnapshot",
    UpdateSnapshot => "updateSnapshot",
    Release => "release",
    CreateSourceFile => "createSourceFile",
    RetainSourceFile => "retainSourceFile",
    ReleaseSourceFile => "releaseSourceFile",
    GetCachedSourceFile => "getCachedSourceFile",
    GetSourceFile => "getSourceFile",
    GetSourceFileNames => "getSourceFileNames",
    GetSourceFileMetadata => "getSourceFileMetadata",
    GetSemanticDiagnostics => "getSemanticDiagnostics",
    GetSyntacticDiagnostics => "getSyntacticDiagnostics",
    GetTypeAtPosition => "getTypeAtPosition",
    GetTypeAtLocation => "getTypeAtLocation",
    GetTypeOfSymbol => "getTypeOfSymbol",
    GetDeclaredTypeOfSymbol => "getDeclaredTypeOfSymbol",
    TypeToString => "typeToString",
    GetSymbolAtPosition => "getSymbolAtPosition",
    GetSymbolAtLocation => "getSymbolAtLocation",
    GetSymbolOfNode => "getSymbolOfNode",
    GetDocumentationComment => "getDocumentationComment",
    GetJsDocTags => "getJsDocTags",
    GetReferencedSymbolsForNode => "getReferencedSymbolsForNode",
    GetCompletionsAtPosition => "getCompletionsAtPosition",
    GetDefaultProjectForFile => "getDefaultProjectForFile",
}

/// One typed outbound request. Construction does not send anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRequest {
    method: NativeMethod,
    params: RawPayload,
}

impl NativeRequest {
    /// Creates a request from an allow-listed method and its params.
    #[must_use]
    pub const fn new(method: NativeMethod, params: RawPayload) -> Self {
        Self { method, params }
    }

    /// Returns the typed method.
    #[must_use]
    pub const fn method(&self) -> NativeMethod {
        self.method
    }

    /// Returns the raw params value.
    #[must_use]
    pub const fn params(&self) -> &RawPayload {
        &self.params
    }
}

/// Failure to decode a response shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ShapeError {
    /// The response was not a JSON object.
    #[error("response is not a JSON object")]
    NotAnObject,
    /// The response was not a JSON array.
    #[error("response is not a JSON array")]
    NotAnArray,
    /// The response was not a JSON string.
    #[error("response is not a JSON string")]
    NotAString,
    /// A required member was absent.
    #[error("response is missing required field {field}")]
    MissingField {
        /// Wire field name.
        field: &'static str,
    },
    /// A member was present with an invalid type or value.
    #[error("response field {field} has an invalid value")]
    InvalidField {
        /// Wire field name.
        field: &'static str,
    },
    /// `initialize` returned the excluded 7.0.2 member
    /// `useCaseSensitiveFileNames`.
    #[error("initialize returned the excluded 7.0.2 shape")]
    LegacyInitializeShape,
    /// `initialize` reported a case sensitivity other than the frozen `1`.
    #[error("initialize caseSensitivity must be 1, got {observed}")]
    UnsupportedCaseSensitivity {
        /// Value reported by the engine.
        observed: i64,
    },
}

impl ShapeError {
    /// Wraps this shape failure in a protocol error with a bounded message.
    #[must_use]
    pub fn into_native_error(self) -> crate::native::NativeError {
        crate::native::NativeError::Protocol(self.to_string())
    }
}

/// Object member lookup that distinguishes "missing" from "wrong type".
fn required<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Value, ShapeError> {
    object.get(field).ok_or(ShapeError::MissingField { field })
}

/// Optional object member that rejects an unexpected null.
fn optional<'a>(object: &'a Map<String, Value>, field: &'static str) -> Option<&'a Value> {
    object.get(field).filter(|value| !value.is_null())
}

/// Result of the `initialize` handshake.
///
/// The frozen contract has no protocol version negotiation: `7.1.0-beta`
/// returns only `caseSensitivity` and `currentDirectory`, and the presence of
/// `useCaseSensitiveFileNames` identifies the excluded `7.0.2` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializeResult {
    case_sensitivity: i64,
    current_directory: String,
}

impl InitializeResult {
    /// Parses and validates the `initialize` result object.
    ///
    /// # Errors
    ///
    /// Rejects the `7.0.2` shape, missing or mistyped members and an empty
    /// `currentDirectory`.
    pub fn from_value(value: &Value) -> Result<Self, ShapeError> {
        let Some(object) = value.as_object() else {
            return Err(ShapeError::NotAnObject);
        };

        if object.contains_key("useCaseSensitiveFileNames") {
            return Err(ShapeError::LegacyInitializeShape);
        }

        let case_sensitivity =
            required(object, "caseSensitivity")?
                .as_i64()
                .ok_or(ShapeError::InvalidField {
                    field: "caseSensitivity",
                })?;
        if case_sensitivity != 1 {
            return Err(ShapeError::UnsupportedCaseSensitivity {
                observed: case_sensitivity,
            });
        }

        let current_directory = required(object, "currentDirectory")?
            .as_str()
            .filter(|directory| !directory.is_empty())
            .ok_or(ShapeError::InvalidField {
                field: "currentDirectory",
            })?;

        Ok(Self {
            case_sensitivity,
            current_directory: current_directory.to_owned(),
        })
    }

    /// Returns the engine case-sensitivity number.
    #[must_use]
    pub const fn case_sensitivity(&self) -> i64 {
        self.case_sensitivity
    }

    /// Returns whether the engine reported case-sensitive paths (`1`).
    #[must_use]
    pub const fn is_case_sensitive(&self) -> bool {
        self.case_sensitivity == 1
    }

    /// Returns the engine working directory echoed by the handshake.
    #[must_use]
    pub fn current_directory(&self) -> &str {
        &self.current_directory
    }
}

/// One project entry returned by `createSnapshot`/`updateSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProject {
    id: String,
    current_directory: Option<String>,
    dirty: Option<bool>,
}

impl SnapshotProject {
    /// Parses one project entry; unknown fields are ignored.
    ///
    /// # Errors
    ///
    /// Requires a string `id`; optional members must have their documented
    /// types when present.
    pub fn from_value(value: &Value) -> Result<Self, ShapeError> {
        let Some(object) = value.as_object() else {
            return Err(ShapeError::NotAnObject);
        };

        let id = required(object, "id")?
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or(ShapeError::InvalidField { field: "id" })?
            .to_owned();

        let current_directory = optional(object, "currentDirectory")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or(ShapeError::InvalidField {
                        field: "currentDirectory",
                    })
            })
            .transpose()?;

        let dirty = optional(object, "dirty")
            .map(|value| {
                value
                    .as_bool()
                    .ok_or(ShapeError::InvalidField { field: "dirty" })
            })
            .transpose()?;

        Ok(Self {
            id,
            current_directory,
            dirty,
        })
    }

    /// Returns the engine project id.
    ///
    /// Synthetic and inferred project ids are opaque engine identifiers and
    /// must not be used as stable Rust project keys.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the project `currentDirectory` when the engine reports one.
    #[must_use]
    pub fn current_directory(&self) -> Option<&str> {
        self.current_directory.as_deref()
    }

    /// Returns the engine `dirty` flag when present.
    #[must_use]
    pub const fn dirty(&self) -> Option<bool> {
        self.dirty
    }
}

/// Result of `createSnapshot`/`updateSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotResult {
    snapshot: u64,
    projects: Vec<SnapshotProject>,
}

impl SnapshotResult {
    /// Parses a snapshot result.
    ///
    /// `snapshot` is required. `projects` is optional and defaults to empty
    /// because P3 only needs the engine snapshot id; when present it must be
    /// an array of valid project entries.
    /// # Errors
    ///
    /// Rejects a missing or non-integer `snapshot` and malformed projects.
    pub fn from_value(value: &Value) -> Result<Self, ShapeError> {
        let Some(object) = value.as_object() else {
            return Err(ShapeError::NotAnObject);
        };

        let snapshot = required(object, "snapshot")?
            .as_u64()
            .ok_or(ShapeError::InvalidField { field: "snapshot" })?;

        let projects = match optional(object, "projects") {
            None => Vec::new(),
            Some(Value::Array(entries)) => entries
                .iter()
                .map(SnapshotProject::from_value)
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(ShapeError::InvalidField { field: "projects" });
            }
        };

        Ok(Self { snapshot, projects })
    }

    /// Returns the engine snapshot id.
    ///
    /// This is an engine identifier, not the Rust snapshot revision; callers
    /// must bind it to a session generation and Rust revision before use.
    #[must_use]
    pub const fn snapshot(&self) -> u64 {
        self.snapshot
    }

    /// Returns the project entries in engine order.
    #[must_use]
    pub fn projects(&self) -> &[SnapshotProject] {
        &self.projects
    }
}

/// Result of `getSourceFileNames`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileNames {
    files: Vec<String>,
}

impl SourceFileNames {
    /// Parses the string array returned by `getSourceFileNames`.
    ///
    /// # Errors
    ///
    /// Rejects non-arrays and non-string entries.
    pub fn from_value(value: &Value) -> Result<Self, ShapeError> {
        let Some(entries) = value.as_array() else {
            return Err(ShapeError::NotAnArray);
        };

        let mut files = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(name) = entry.as_str() else {
                return Err(ShapeError::InvalidField { field: "files" });
            };
            files.push(name.to_owned());
        }

        Ok(Self { files })
    }

    /// Returns the snapshot file names in engine order.
    #[must_use]
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Consumes the result and returns the file names.
    #[must_use]
    pub fn into_files(self) -> Vec<String> {
        self.files
    }
}

/// Opaque engine type handle.
///
/// The numeric id is only meaningful inside the snapshot that produced it;
/// `P4` must bind it to the session/snapshot before sending it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeHandle {
    id: u64,
}

impl TypeHandle {
    /// Parses a non-null type object.
    ///
    /// # Errors
    ///
    /// Requires an object with an unsigned integer `id`.
    pub fn from_value(value: &Value) -> Result<Self, ShapeError> {
        let Some(object) = value.as_object() else {
            return Err(ShapeError::NotAnObject);
        };
        let id = required(object, "id")?
            .as_u64()
            .ok_or(ShapeError::InvalidField { field: "id" })?;
        Ok(Self { id })
    }

    /// Parses a nullable type result: `null` means "no type at this position".
    ///
    /// # Errors
    ///
    /// Propagates [`Self::from_value`] for non-null values.
    pub fn optional(value: &Value) -> Result<Option<Self>, ShapeError> {
        if value.is_null() {
            Ok(None)
        } else {
            Self::from_value(value).map(Some)
        }
    }

    /// Returns the opaque engine type id.
    #[must_use]
    pub const fn id(self) -> u64 {
        self.id
    }
}

/// Text returned by `typeToString` or `getDocumentationComment`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextResponse(String);

impl TextResponse {
    /// Parses a string result.
    ///
    /// # Errors
    ///
    /// Rejects non-string values.
    pub fn from_value(value: &Value) -> Result<Self, ShapeError> {
        value
            .as_str()
            .map(|text| Self(text.to_owned()))
            .ok_or(ShapeError::NotAString)
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the response and returns the text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ALLOWED_METHODS;
    use serde_json::json;

    #[test]
    fn typed_methods_match_the_frozen_allowlist() {
        let names: Vec<&str> = NativeMethod::ALL
            .iter()
            .map(|method| method.as_str())
            .collect();
        assert_eq!(names.as_slice(), ALLOWED_METHODS);
    }

    #[test]
    fn typed_methods_do_not_include_legacy_language_service_names() {
        for legacy in [
            "getQuickInfoAtPosition",
            "getDefinitionAtPosition",
            "getDefinitionAndBoundSpan",
            "getRenameInfo",
            "getEditsForRename",
            "rename",
            "getCompletionEntryDetails",
        ] {
            assert!(
                !NativeMethod::ALL
                    .iter()
                    .any(|method| method.as_str() == legacy),
                "legacy method {legacy} must not be reachable"
            );
        }
    }

    #[test]
    fn initialize_requires_the_7_1_shape() {
        let result = InitializeResult::from_value(&json!({
            "caseSensitivity": 1,
            "currentDirectory": "/sandbox",
            "unknown": {"ignored": true},
        }))
        .expect("valid handshake");

        assert_eq!(result.case_sensitivity(), 1);
        assert!(result.is_case_sensitive());
        assert_eq!(result.current_directory(), "/sandbox");

        assert!(matches!(
            InitializeResult::from_value(&json!({
                "caseSensitivity": 1,
                "currentDirectory": "/sandbox",
                "useCaseSensitiveFileNames": true,
            })),
            Err(ShapeError::LegacyInitializeShape)
        ));
        assert!(matches!(
            InitializeResult::from_value(&json!({"currentDirectory": "/sandbox"})),
            Err(ShapeError::MissingField {
                field: "caseSensitivity"
            })
        ));
        assert!(matches!(
            InitializeResult::from_value(&json!({
                "caseSensitivity": "1",
                "currentDirectory": "/sandbox",
            })),
            Err(ShapeError::InvalidField {
                field: "caseSensitivity"
            })
        ));
        assert!(matches!(
            InitializeResult::from_value(&json!({"caseSensitivity": 1, "currentDirectory": ""})),
            Err(ShapeError::InvalidField {
                field: "currentDirectory"
            })
        ));
        for observed in [0, 2] {
            assert!(matches!(
                InitializeResult::from_value(&json!({
                    "caseSensitivity": observed,
                    "currentDirectory": "/sandbox",
                })),
                Err(ShapeError::UnsupportedCaseSensitivity { observed: found })
                    if found == observed
            ));
        }
        assert!(matches!(
            InitializeResult::from_value(&json!([])),
            Err(ShapeError::NotAnObject)
        ));
    }

    #[test]
    fn snapshot_result_requires_the_snapshot_id() {
        let result = SnapshotResult::from_value(&json!({
            "snapshot": 1,
            "projects": [{
                "id": "/dev/null/synthetic/1",
                "currentDirectory": "/sandbox",
                "dirty": false,
                "parsedCommandLine": {"ignored": true},
            }],
            "operation": {"createdPrograms": ["/dev/null/synthetic/1"]},
        }))
        .expect("valid snapshot result");

        assert_eq!(result.snapshot(), 1);
        assert_eq!(result.projects().len(), 1);
        let project = &result.projects()[0];
        assert_eq!(project.id(), "/dev/null/synthetic/1");
        assert_eq!(project.current_directory(), Some("/sandbox"));
        assert_eq!(project.dirty(), Some(false));

        let incremental =
            SnapshotResult::from_value(&json!({"snapshot": 2, "projects": []})).expect("update");
        assert_eq!(incremental.snapshot(), 2);
        assert_eq!(incremental.projects(), &[][..]);

        let without_projects =
            SnapshotResult::from_value(&json!({"snapshot": 3})).expect("minimal result");
        assert_eq!(without_projects.projects(), &[][..]);

        assert!(matches!(
            SnapshotResult::from_value(&json!({"projects": []})),
            Err(ShapeError::MissingField { field: "snapshot" })
        ));
        assert!(matches!(
            SnapshotResult::from_value(&json!({"snapshot": "1"})),
            Err(ShapeError::InvalidField { field: "snapshot" })
        ));
        assert!(matches!(
            SnapshotResult::from_value(&json!({"snapshot": 1, "projects": {}})),
            Err(ShapeError::InvalidField { field: "projects" })
        ));
        assert!(matches!(
            SnapshotResult::from_value(&json!({"snapshot": 1, "projects": [{"id": 7}]})),
            Err(ShapeError::InvalidField { field: "id" })
        ));
        assert!(matches!(
            SnapshotResult::from_value(
                &json!({"snapshot": 1, "projects": [{"id": "p", "dirty": "no"}]})
            ),
            Err(ShapeError::InvalidField { field: "dirty" })
        ));
    }

    #[test]
    fn source_file_names_are_strings() {
        let names = SourceFileNames::from_value(&json!(["/sandbox/main.ts", "/sandbox/a.ts"]))
            .expect("file names");
        assert_eq!(names.files().len(), 2);
        assert_eq!(names.into_files()[0], "/sandbox/main.ts");

        assert!(matches!(
            SourceFileNames::from_value(&json!({"files": []})),
            Err(ShapeError::NotAnArray)
        ));
        assert!(matches!(
            SourceFileNames::from_value(&json!(["ok", 7])),
            Err(ShapeError::InvalidField { field: "files" })
        ));
    }

    #[test]
    fn type_handles_and_text_results_are_minimal() {
        let handle = TypeHandle::from_value(&json!({
            "id": 65,
            "flags": 1_048_576,
            "symbol": {"file": "1", "id": 1},
        }))
        .expect("type handle");
        assert_eq!(handle.id(), 65);
        assert_eq!(TypeHandle::optional(&Value::Null).expect("null"), None);
        assert_eq!(
            TypeHandle::optional(&json!({"id": 9})).expect("handle"),
            Some(TypeHandle { id: 9 })
        );

        assert!(matches!(
            TypeHandle::from_value(&json!({"flags": 1})),
            Err(ShapeError::MissingField { field: "id" })
        ));
        assert!(matches!(
            TypeHandle::from_value(&json!({"id": -1})),
            Err(ShapeError::InvalidField { field: "id" })
        ));

        let text = TextResponse::from_value(&json!("User")).expect("text");
        assert_eq!(text.as_str(), "User");
        assert_eq!(text.into_string(), "User");
        assert!(matches!(
            TextResponse::from_value(&json!(null)),
            Err(ShapeError::NotAString)
        ));
    }

    #[test]
    fn shape_errors_become_bounded_protocol_errors() {
        let error = ShapeError::LegacyInitializeShape.into_native_error();
        assert_eq!(
            error.to_string(),
            "native protocol violation: initialize returned the excluded 7.0.2 shape"
        );
    }
}
