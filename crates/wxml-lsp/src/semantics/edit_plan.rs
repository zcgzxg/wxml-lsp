//! Unified validation and serialization for every edit-bearing result.
//!
//! A [`EditPlan`] is built from exact source tokens, then validated as a whole
//! against the current workspace/host state. Any failure rejects the entire
//! plan: the LSP layer never returns a partial edit and never writes a file.
//!
//! The checks cover, in order:
//!
//! 1. every target belongs to the query document's project and is not in a
//!    package/build directory or a generated declaration file;
//! 2. the target text is the expected revision (client version for open
//!    documents, a fresh host content read for closed ones);
//! 3. each range is valid, on character boundaries and still contains exactly
//!    the expected original text;
//! 4. edits are deduplicated and non-overlapping, and zero-width insertion
//!    anchors are unique;
//! 5. each replacement satisfies its token grammar and keeps surrounding
//!    prefixes (quotes, `bind:`/`catch:`, attribute syntax) untouched;
//! 6. all ranges serialize to valid UTF-16 LSP ranges.
//!
//! Only `WorkspaceEdit.documentChanges` is produced. Closed files use a
//! `null` client version and are re-read before rendering; an internal
//! revision is never presented as a client version.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use wxml_project::sources::EXCLUDED_DIRECTORIES;
use wxml_text::SourceRange;

use super::position::Utf16Cursor;
use crate::server::Server;
use crate::workspace::Document;

/// Why one edit plan was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlanError {
    /// The edit would leave the project or a writable source set.
    Path,
    /// The target changed since the analysis snapshot.
    Stale,
    /// The expected original text did not match.
    Content,
    /// Two edits collide or one insertion anchor is not unique.
    Overlap,
    /// The replacement does not satisfy the target token grammar.
    Name,
    /// A range could not be represented as UTF-16.
    Range,
}

impl PlanError {
    /// Short client-safe explanation, without a path or file content.
    pub(crate) const fn message(&self) -> &'static str {
        match self {
            Self::Path => "the edit target is not a writable project file",
            Self::Stale => "the target changed while the edit was being prepared",
            Self::Content => "the target text no longer matches the analysis snapshot",
            Self::Overlap => "the edit set is not unique or contains overlapping ranges",
            Self::Name => "the new name is not valid at every target",
            Self::Range => "an edit range cannot be represented in UTF-16",
        }
    }
}

/// Grammar a replacement token must satisfy.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditRule {
    /// A plain JavaScript/TypeScript identifier.
    Identifier,
    /// A template name: letters, digits, `_` and `-`, not starting with `-`.
    Template,
    /// A WXML attribute name: camelCase or kebab-case.
    AttributeName,
    /// A CSS class name fragment.
    Class,
    /// One dot-separated data path segment.
    PathSegment,
    /// A preformatted insertion that the caller already validated.
    Preformatted,
}

impl EditRule {
    fn accepts(self, text: &str) -> bool {
        match self {
            Self::Identifier => is_identifier(text),
            Self::Template => {
                let mut chars = text.chars();
                let Some(first) = chars.next() else {
                    return false;
                };
                (first.is_ascii_alphabetic() || first == '_')
                    && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            }
            Self::AttributeName => {
                let mut chars = text.chars();
                let Some(first) = chars.next() else {
                    return false;
                };
                first.is_ascii_alphabetic()
                    && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            }
            Self::Class => {
                let mut chars = text.chars();
                let Some(first) = chars.next() else {
                    return false;
                };
                (first.is_ascii_alphabetic() || first == '_')
                    && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            }
            Self::PathSegment => {
                !text.is_empty()
                    && !text.contains(['.', '[', ']', '\'', '"', '`', '\\'])
                    && !text.chars().any(char::is_whitespace)
                    && !text.chars().any(char::is_control)
            }
            Self::Preformatted => !text.is_empty(),
        }
    }
}

/// Returns `true` for a conservative ASCII JavaScript identifier.
pub(crate) fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
}

/// One exact replacement in an original file.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PlannedEdit {
    range: SourceRange,
    expected: String,
    replacement: String,
    rule: EditRule,
}

/// Edits for one target file.
#[derive(Clone, Debug, Default)]
struct PlannedFile {
    edits: Vec<PlannedEdit>,
}

/// Edits prepared for one query and their source project.
#[derive(Debug)]
pub(crate) struct EditPlan {
    source: wxml_types::FileId,
    files: BTreeMap<wxml_types::FileId, PlannedFile>,
}

impl EditPlan {
    /// Creates a plan for edits issued from `source`.
    pub(crate) const fn new(source: wxml_types::FileId) -> Self {
        Self {
            source,
            files: BTreeMap::new(),
        }
    }

    /// Adds one replacement. `expected` must be the exact current slice.
    pub(crate) fn add(
        &mut self,
        file: &wxml_types::FileId,
        range: SourceRange,
        expected: impl Into<String>,
        replacement: impl Into<String>,
        rule: EditRule,
    ) -> Result<(), PlanError> {
        if file.project() != self.source.project() {
            return Err(PlanError::Path);
        }
        let planned = PlannedEdit {
            range,
            expected: expected.into(),
            replacement: replacement.into(),
            rule,
        };
        let entry = self.files.entry(file.clone()).or_default();
        if let Some(existing) = entry.edits.iter().find(|edit| edit.range == planned.range) {
            if existing.expected == planned.expected
                && existing.replacement == planned.replacement
                && existing.rule == planned.rule
            {
                return Ok(());
            }
            return Err(PlanError::Overlap);
        }
        entry.edits.push(planned);
        Ok(())
    }

    /// Returns `true` when the plan has no edits.
    pub(crate) fn is_empty(&self) -> bool {
        self.files.values().all(|file| file.edits.is_empty())
    }

    /// Validates the complete plan and renders `WorkspaceEdit.documentChanges`.
    pub(crate) fn document_changes(self, server: &Server) -> Result<Value, PlanError> {
        if self.is_empty() {
            return Err(PlanError::Content);
        }
        let mut changes = Vec::with_capacity(self.files.len());
        for (file, planned) in self.files {
            if !editable_path(file.path()) {
                return Err(PlanError::Path);
            }
            let target = match server.workspace.get_by_file(&file) {
                Some(document) => {
                    if server.host.is_project_open()
                        && server.host.uri_for(&file).is_some()
                        && !server.host.writable_target(&file)
                    {
                        return Err(PlanError::Path);
                    }
                    PlanTarget::Open(OpenTarget::from_document(document))
                }
                None => PlanTarget::Closed(ClosedTarget::load(server, &file)?),
            };
            let text = target.text();
            let mut edits = planned.edits;
            for edit in &edits {
                if !edit.rule.accepts(&edit.replacement) {
                    return Err(PlanError::Name);
                }
                if !range_is_valid(text, edit.range) {
                    return Err(PlanError::Range);
                }
                let start = usize::try_from(edit.range.start()).map_err(|_| PlanError::Range)?;
                let end = usize::try_from(edit.range.end()).map_err(|_| PlanError::Range)?;
                let Some(actual) = text.get(start..end) else {
                    return Err(PlanError::Range);
                };
                if actual != edit.expected {
                    return Err(PlanError::Content);
                }
            }
            edits.sort_by_key(|edit| (edit.range.start(), edit.range.end()));
            for pair in edits.windows(2) {
                let left = pair[0].range;
                let right = pair[1].range;
                if left.end() > right.start() {
                    return Err(PlanError::Overlap);
                }
                if left.is_empty() && right.is_empty() && left.start() == right.start() {
                    return Err(PlanError::Overlap);
                }
            }
            let mut serialized = Vec::with_capacity(edits.len());
            let mut cursor = Utf16Cursor::new(text);
            for edit in &edits {
                let (Some(start), Some(end)) = (
                    cursor.position(edit.range.start()),
                    cursor.position(edit.range.end()),
                ) else {
                    return Err(PlanError::Range);
                };
                serialized.push(json!({
                    "range": {
                        "start": crate::handlers::position_value(start),
                        "end": crate::handlers::position_value(end),
                    },
                    "newText": edit.replacement,
                }));
            }
            changes.push(json!({
                "textDocument": {
                    "uri": target.uri(),
                    "version": target.version(),
                },
                "edits": serialized,
            }));
        }
        Ok(json!({"documentChanges": changes}))
    }
}

/// Generated/source-of-truth directories that must never receive edits.
const EDIT_EXCLUDED_DIRECTORIES: [&str; 3] = ["generated", "__generated__", "gen"];

/// Returns `true` for a path inside an editable project source tree.
fn editable_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".d.ts") || lower.ends_with(".d.mts") || lower.ends_with(".d.cts") {
        return false;
    }
    !path.split('/').any(|component| {
        EXCLUDED_DIRECTORIES
            .iter()
            .chain(EDIT_EXCLUDED_DIRECTORIES.iter())
            .any(|excluded| component.eq_ignore_ascii_case(excluded))
    })
}

fn range_is_valid(text: &str, range: SourceRange) -> bool {
    let Ok(start) = usize::try_from(range.start()) else {
        return false;
    };
    let Ok(end) = usize::try_from(range.end()) else {
        return false;
    };
    start <= end && end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end)
}

/// A target document resolved for validation and serialization.
enum PlanTarget<'a> {
    /// An open client buffer.
    Open(OpenTarget<'a>),
    /// A closed project file.
    Closed(ClosedTarget),
}

impl PlanTarget<'_> {
    fn text(&self) -> &str {
        match self {
            Self::Open(target) => target.text(),
            Self::Closed(target) => target.text(),
        }
    }

    fn uri(&self) -> &str {
        match self {
            Self::Open(target) => target.uri(),
            Self::Closed(target) => target.uri(),
        }
    }

    fn version(&self) -> Value {
        match self {
            Self::Open(target) => target.version(),
            Self::Closed(target) => target.version(),
        }
    }
}

/// An open document: the client version and its own snapshot are authoritative.
struct OpenTarget<'a> {
    document: &'a Document,
}

impl<'a> OpenTarget<'a> {
    const fn from_document(document: &'a Document) -> Self {
        Self { document }
    }

    fn text(&self) -> &str {
        self.document.text()
    }

    fn uri(&self) -> &str {
        self.document.uri()
    }

    fn version(&self) -> Value {
        json!(self.document.client_version().get())
    }
}

/// A closed project file: re-read from the host and rendered with a null version.
struct ClosedTarget {
    uri: String,
    text: String,
}

impl ClosedTarget {
    fn load(server: &Server, file: &wxml_types::FileId) -> Result<Self, PlanError> {
        if !server.host.writable_target(file) {
            return Err(PlanError::Path);
        }
        let Some(uri) = server.host.uri_for(file) else {
            return Err(PlanError::Path);
        };
        let Some(text) = server.host.file_text(file) else {
            return Err(PlanError::Stale);
        };
        Ok(Self {
            uri,
            text: text.to_string(),
        })
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn uri(&self) -> &str {
        &self.uri
    }

    #[allow(clippy::unused_self)] // keeps the PlanTarget call site uniform
    const fn version(&self) -> Value {
        Value::Null
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use wxml_types::{FileId, ProjectId};

    fn file() -> FileId {
        FileId::new(ProjectId::new("test"), "pages/index/index.wxml").expect("file")
    }

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange::from_bounds(start, end).expect("range")
    }

    #[test]
    fn rejects_invalid_replacement_grammar() {
        assert!(EditRule::Identifier.accepts("nextField"));
        assert!(!EditRule::Identifier.accepts("next-field"));
        assert!(EditRule::Template.accepts("row-item"));
        assert!(!EditRule::Template.accepts("-row"));
        assert!(EditRule::PathSegment.accepts("next"));
        assert!(!EditRule::PathSegment.accepts("a.b"));
        assert!(!EditRule::PathSegment.accepts("a[b]"));
    }

    #[test]
    fn rejects_package_and_generated_targets() {
        assert!(editable_path("pages/index/index.ts"));
        assert!(!editable_path("node_modules/pkg/index.ts"));
        assert!(!editable_path("miniprogram_npm/pkg/index.js"));
        assert!(!editable_path("generated/types.d.ts"));
        assert!(!editable_path("dist/bundle.js"));
    }

    #[test]
    fn deduplicates_identical_edits_and_rejects_conflicts() {
        let mut plan = EditPlan::new(file());
        plan.add(&file(), range(1, 3), "ab", "cd", EditRule::Identifier)
            .expect("first");
        plan.add(&file(), range(1, 3), "ab", "cd", EditRule::Identifier)
            .expect("duplicate");
        assert_eq!(plan.files.get(&file()).expect("file").edits.len(), 1);
        assert_eq!(
            plan.add(&file(), range(1, 3), "ab", "xy", EditRule::Identifier),
            Err(PlanError::Overlap)
        );
        assert_eq!(
            plan.add(
                &FileId::new(ProjectId::new("other"), "pages/index/index.ts").expect("other"),
                range(1, 2),
                "a",
                "b",
                EditRule::Identifier
            ),
            Err(PlanError::Path)
        );
    }

    #[test]
    fn rejects_an_empty_plan() {
        let plan = EditPlan::new(file());
        assert!(plan.is_empty());
    }
}
