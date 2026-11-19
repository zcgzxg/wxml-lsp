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

/// Largest number of edits accepted in one plan.
pub(crate) const MAX_PLAN_EDITS: usize = 4096;

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
        let total: usize = self.files.values().map(|file| file.edits.len()).sum();
        if total >= MAX_PLAN_EDITS {
            return Err(PlanError::Overlap);
        }
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

/// Fail-closed post-edit recheck seam for future native-backed renames.
///
/// Contract §9.9 item 7 requires every candidate edit set to be applied to an
/// isolated in-memory snapshot, reparsed and typechecked before any edit is
/// published. This module implements that reusable seam. It is deliberately
/// staged: handlers are not wired to it in this change.
#[allow(dead_code)] // staged for the native-backed rename wiring (contract §9.9 item 7)
pub(crate) mod recheck {
    use wxml_text::SourceRange;

    use super::{MAX_PLAN_EDITS, range_is_valid};
    use crate::semantics::position::Utf16Cursor;

    /// Maximum number of edits accepted by one post-edit recheck.
    pub(crate) const MAX_RECHECK_EDITS: usize = MAX_PLAN_EDITS;

    /// Maximum total replacement bytes accepted by one post-edit recheck.
    ///
    /// The budget is measured after exact duplicate edits are removed.
    pub(crate) const MAX_RECHECK_REPLACEMENT_BYTES: usize = 1024 * 1024;

    /// Maximum original text bytes accepted by one post-edit recheck.
    pub(crate) const MAX_RECHECK_SOURCE_BYTES: usize = 8 * 1024 * 1024;

    /// Maximum applied snapshot bytes accepted by one post-edit recheck.
    ///
    /// This matches the inbound document budget: a recheck never builds a
    /// snapshot larger than the original source limit, even when a replacement
    /// would otherwise grow the file.
    pub(crate) const MAX_RECHECK_SNAPSHOT_BYTES: usize = MAX_RECHECK_SOURCE_BYTES;

    /// Maximum diagnostics retained in one recheck report.
    pub(crate) const MAX_RECHECK_DIAGNOSTICS: usize = 32;

    /// Maximum bytes retained for one diagnostic message.
    pub(crate) const MAX_RECHECK_DIAGNOSTIC_BYTES: usize = 4 * 1024;

    /// One exact replacement applied to an in-memory recheck snapshot.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) struct RecheckEdit<'a> {
        /// Byte range in the original text.
        pub(crate) range: SourceRange,
        /// Exact original slice expected at `range`.
        pub(crate) expected: &'a str,
        /// Replacement text.
        pub(crate) replacement: &'a str,
    }

    impl<'a> RecheckEdit<'a> {
        /// Creates one recheck replacement.
        pub(crate) const fn new(
            range: SourceRange,
            expected: &'a str,
            replacement: &'a str,
        ) -> Self {
            Self {
                range,
                expected,
                replacement,
            }
        }
    }

    /// Stage that produced recheck diagnostics.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum RecheckStage {
        /// WXML markup or expression parsing.
        Parse,
        /// Optional typecheck provider.
        TypeCheck,
    }

    /// One bounded diagnostic in the applied snapshot.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(crate) struct RecheckDiagnostic {
        /// Byte range in the applied snapshot.
        pub(crate) range: SourceRange,
        /// Short, client-safe description.
        pub(crate) message: String,
    }

    /// Why a complete edit set could not be applied.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum InvalidPlanCause {
        /// The edit set contained no edits.
        Empty,
        /// The raw edit count exceeded [`MAX_RECHECK_EDITS`].
        TooManyEdits,
        /// The original text exceeded [`MAX_RECHECK_SOURCE_BYTES`].
        SourceTooLarge,
        /// The applied text would exceed [`MAX_RECHECK_SNAPSHOT_BYTES`].
        SnapshotTooLarge,
        /// The deduplicated replacement text exceeded
        /// [`MAX_RECHECK_REPLACEMENT_BYTES`].
        ReplacementTooLarge,
        /// A range was out of bounds or not on a character boundary.
        Range,
        /// Two edits overlapped or shared a zero-width anchor.
        Overlap,
        /// A zero-width insertion would split a CRLF pair.
        CrlfBoundary,
        /// A zero-width insertion would land before a leading BOM.
        BomBoundary,
    }

    /// Result of applying and rechecking a candidate edit set.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(crate) enum RecheckReport {
        /// The complete edit set applied and every available check passed.
        Clean,
        /// The applied text has parse or type errors.
        SyntaxErrors {
            /// Stage that reported the errors.
            stage: RecheckStage,
            /// Bounded diagnostics in the applied text.
            diagnostics: Vec<RecheckDiagnostic>,
            /// `true` when the diagnostic list or a message was shortened.
            truncated: bool,
            /// Number of diagnostic entries reported by the stage before this
            /// list was shortened.
            total: usize,
        },
        /// At least one expected original slice was stale.
        StaleEdit {
            /// Index of the stale edit in the caller's input slice.
            edit: usize,
        },
        /// The edit set could not be applied atomically in bounds.
        InvalidPlan {
            /// Why the set was rejected.
            cause: InvalidPlanCause,
            /// Index of the offending edit, when one edit caused the failure.
            edit: Option<usize>,
        },
        /// Parsing succeeded but no typecheck result was available.
        CheckUnavailable,
    }

    /// Outcome of the optional typecheck stage.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(crate) enum TypeCheckOutcome {
        /// The applied snapshot has no type errors.
        Clean,
        /// Type errors in the applied snapshot.
        ///
        /// An empty list is treated as unavailable, never as clean. The recheck
        /// validates every retained range against the snapshot and bounds
        /// message length; an invalid range makes the whole result unavailable.
        Errors(Vec<RecheckDiagnostic>),
        /// No typecheck result is available.
        Unavailable,
    }

    /// Optional, text-only typecheck stage for a post-edit recheck.
    ///
    /// This stage receives only the fully applied snapshot text for one file.
    /// It is deliberately not project-aware: it cannot resolve imports, sibling
    /// files or TypeScript project state. A project-aware typechecker will be a
    /// separate contract; this seam stays deterministic and file-local.
    ///
    /// Implementations must use only `text`; no file or workspace state is
    /// available to this stage. Diagnostic ranges are validated against the
    /// snapshot and messages are bounded by the recheck.
    pub(crate) trait TypeCheckProvider {
        /// Checks the fully applied snapshot.
        fn check(&self, text: &str) -> TypeCheckOutcome;
    }

    /// Explicit no-op provider: every recheck reports
    /// [`RecheckReport::CheckUnavailable`].
    #[derive(Clone, Copy, Debug, Default)]
    pub(crate) struct UnavailableTypeCheck;

    impl TypeCheckProvider for UnavailableTypeCheck {
        fn check(&self, _text: &str) -> TypeCheckOutcome {
            TypeCheckOutcome::Unavailable
        }
    }

    /// Why applying an edit set to an in-memory snapshot failed.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ApplyFailure {
        /// An expected original slice was stale.
        Stale {
            /// Index in the caller's input slice.
            edit: usize,
        },
        /// The edit set could not be applied.
        Invalid {
            /// Why the set was rejected.
            cause: InvalidPlanCause,
            /// Index in the caller's input slice, when one edit applies.
            edit: Option<usize>,
        },
    }

    impl ApplyFailure {
        const fn invalid(cause: InvalidPlanCause, edit: Option<usize>) -> Self {
            Self::Invalid { cause, edit }
        }
    }

    /// A fully applied in-memory text snapshot.
    #[derive(Debug)]
    struct AppliedSnapshot {
        text: String,
    }

    impl AppliedSnapshot {
        fn text(&self) -> &str {
            &self.text
        }
    }

    /// Provider diagnostics after range validation and message bounding.
    #[derive(Debug)]
    struct BoundedDiagnostics {
        diagnostics: Vec<RecheckDiagnostic>,
        truncated: bool,
        total: usize,
    }

    /// Applies `edits` to an in-memory copy of `original` and rechecks it.
    ///
    /// No file, host or workspace state is read or written. Every edit is
    /// validated before the snapshot is built, so a rejected set never produces
    /// a partially applied document.
    ///
    /// The resulting text must parse without errors. Typechecking is opt-in:
    /// when `typecheck` is `None`, reports
    /// [`TypeCheckOutcome::Unavailable`], or returns an empty error list, the
    /// report is [`RecheckReport::CheckUnavailable`], never
    /// [`RecheckReport::Clean`].
    pub(crate) fn recheck_edit_snapshot(
        original: &str,
        edits: &[RecheckEdit<'_>],
        typecheck: Option<&dyn TypeCheckProvider>,
    ) -> RecheckReport {
        let snapshot = match apply_recheck_edits(original, edits) {
            Ok(snapshot) => snapshot,
            Err(ApplyFailure::Stale { edit }) => return RecheckReport::StaleEdit { edit },
            Err(ApplyFailure::Invalid { cause, edit }) => {
                return RecheckReport::InvalidPlan { cause, edit };
            }
        };
        let parsed = wxml_syntax::parse(snapshot.text());
        if !parsed.is_clean() {
            let total = parsed.errors.len();
            let truncated = parsed.truncated || total > MAX_RECHECK_DIAGNOSTICS;
            let diagnostics = parsed
                .errors
                .iter()
                .take(MAX_RECHECK_DIAGNOSTICS)
                .map(|error| RecheckDiagnostic {
                    range: SourceRange::new(error.range),
                    message: error.message.to_owned(),
                })
                .collect();
            return RecheckReport::SyntaxErrors {
                stage: RecheckStage::Parse,
                diagnostics,
                truncated,
                total,
            };
        }
        let Some(provider) = typecheck else {
            return RecheckReport::CheckUnavailable;
        };
        match provider.check(snapshot.text()) {
            TypeCheckOutcome::Clean => RecheckReport::Clean,
            TypeCheckOutcome::Errors(diagnostics) if diagnostics.is_empty() => {
                RecheckReport::CheckUnavailable
            }
            TypeCheckOutcome::Errors(diagnostics) => {
                let Some(bounded) = bound_provider_diagnostics(snapshot.text(), diagnostics) else {
                    return RecheckReport::CheckUnavailable;
                };
                RecheckReport::SyntaxErrors {
                    stage: RecheckStage::TypeCheck,
                    diagnostics: bounded.diagnostics,
                    truncated: bounded.truncated,
                    total: bounded.total,
                }
            }
            TypeCheckOutcome::Unavailable => RecheckReport::CheckUnavailable,
        }
    }

    /// Sorts edits by range and removes exact duplicates.
    ///
    /// Each entry keeps the index of its first occurrence in the caller's input
    /// slice so a later failure can point at the original edit.
    fn ordered_unique_edits<'a>(edits: &[RecheckEdit<'a>]) -> Vec<(usize, RecheckEdit<'a>)> {
        let mut ordered: Vec<(usize, RecheckEdit<'a>)> =
            edits.iter().copied().enumerate().collect();
        ordered.sort_by_key(|(_, edit)| (edit.range.start(), edit.range.end()));
        ordered.dedup_by(|(_, left), (_, right)| {
            left.range == right.range
                && left.expected == right.expected
                && left.replacement == right.replacement
        });
        ordered
    }

    /// Validates every unique edit and returns its replacement and removed
    /// byte totals.
    fn validate_unique_edits(
        original: &str,
        ordered: &[(usize, RecheckEdit<'_>)],
    ) -> Result<(usize, usize), ApplyFailure> {
        let mut replacement_bytes = 0_usize;
        let mut removed_bytes = 0_usize;
        for (index, edit) in ordered {
            let start = usize::try_from(edit.range.start())
                .map_err(|_| ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)))?;
            let end = usize::try_from(edit.range.end())
                .map_err(|_| ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)))?;
            if start > end || end > original.len() {
                return Err(ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)));
            }
            if !original.is_char_boundary(start) || !original.is_char_boundary(end) {
                return Err(ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)));
            }
            let Some(actual) = original.get(start..end) else {
                return Err(ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)));
            };
            if actual != edit.expected {
                return Err(ApplyFailure::Stale { edit: *index });
            }
            replacement_bytes = replacement_bytes
                .checked_add(edit.replacement.len())
                .ok_or_else(|| {
                    ApplyFailure::invalid(InvalidPlanCause::ReplacementTooLarge, Some(*index))
                })?;
            if replacement_bytes > MAX_RECHECK_REPLACEMENT_BYTES {
                return Err(ApplyFailure::invalid(
                    InvalidPlanCause::ReplacementTooLarge,
                    Some(*index),
                ));
            }
            removed_bytes = removed_bytes
                .checked_add(end - start)
                .ok_or_else(|| ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)))?;
        }
        Ok((replacement_bytes, removed_bytes))
    }

    /// Rejects overlap and zero-width anchors that are not stable boundaries.
    fn validate_layout(
        original: &str,
        ordered: &[(usize, RecheckEdit<'_>)],
    ) -> Result<(), ApplyFailure> {
        let mut previous_end = None;
        let mut previous_empty_start = None;
        for (index, edit) in ordered {
            if previous_end.is_some_and(|end| end > edit.range.start()) {
                return Err(ApplyFailure::invalid(
                    InvalidPlanCause::Overlap,
                    Some(*index),
                ));
            }
            if edit.range.is_empty() {
                if previous_empty_start == Some(edit.range.start()) {
                    return Err(ApplyFailure::invalid(
                        InvalidPlanCause::Overlap,
                        Some(*index),
                    ));
                }
                if let Some(cause) = zero_width_anchor_failure(original, edit.range.start()) {
                    return Err(ApplyFailure::invalid(cause, Some(*index)));
                }
                previous_empty_start = Some(edit.range.start());
            }
            previous_end = Some(edit.range.end());
        }
        Ok(())
    }

    /// Rejects zero-width insertions that would land before a leading BOM or
    /// between the bytes of a CRLF pair.
    fn zero_width_anchor_failure(text: &str, offset: u32) -> Option<InvalidPlanCause> {
        let Ok(offset) = usize::try_from(offset) else {
            return Some(InvalidPlanCause::Range);
        };
        if offset == 0 && text.starts_with('\u{feff}') {
            return Some(InvalidPlanCause::BomBoundary);
        }
        let bytes = text.as_bytes();
        let previous = offset.checked_sub(1).and_then(|index| bytes.get(index));
        let next = bytes.get(offset);
        if previous == Some(&b'\r') && next == Some(&b'\n') {
            return Some(InvalidPlanCause::CrlfBoundary);
        }
        None
    }

    /// Defense-in-depth UTF-16 coordinate validation.
    ///
    /// The character-boundary checks above already reject ranges that split a
    /// scalar value. This second pass keeps the seam on the same bounded UTF-16
    /// cursor used by the LSP layer.
    fn validate_utf16(
        original: &str,
        ordered: &[(usize, RecheckEdit<'_>)],
    ) -> Result<(), ApplyFailure> {
        let mut cursor = Utf16Cursor::with_limit(original, original.len());
        for (index, edit) in ordered {
            if cursor.position(edit.range.start()).is_none()
                || cursor.position(edit.range.end()).is_none()
            {
                return Err(ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)));
            }
        }
        Ok(())
    }

    /// Bounds a provider diagnostic list and each message.
    fn bound_provider_diagnostics(
        text: &str,
        diagnostics: Vec<RecheckDiagnostic>,
    ) -> Option<BoundedDiagnostics> {
        let total = diagnostics.len();
        let mut truncated = total > MAX_RECHECK_DIAGNOSTICS;
        let mut bounded = Vec::with_capacity(total.min(MAX_RECHECK_DIAGNOSTICS));
        for mut diagnostic in diagnostics.into_iter().take(MAX_RECHECK_DIAGNOSTICS) {
            if !range_is_valid(text, diagnostic.range) {
                return None;
            }
            let (message, shortened) = bound_message(diagnostic.message);
            diagnostic.message = message;
            truncated |= shortened;
            bounded.push(diagnostic);
        }
        Some(BoundedDiagnostics {
            diagnostics: bounded,
            truncated,
            total,
        })
    }

    /// Truncates a diagnostic message on a character boundary.
    fn bound_message(mut message: String) -> (String, bool) {
        if message.len() <= MAX_RECHECK_DIAGNOSTIC_BYTES {
            return (message, false);
        }
        let mut cut = MAX_RECHECK_DIAGNOSTIC_BYTES;
        while cut > 0 && !message.is_char_boundary(cut) {
            cut -= 1;
        }
        message.truncate(cut);
        (message, true)
    }

    /// Validates the complete edit set, then builds the applied snapshot.
    fn apply_recheck_edits(
        original: &str,
        edits: &[RecheckEdit<'_>],
    ) -> Result<AppliedSnapshot, ApplyFailure> {
        if original.len() > MAX_RECHECK_SOURCE_BYTES {
            return Err(ApplyFailure::invalid(
                InvalidPlanCause::SourceTooLarge,
                None,
            ));
        }
        if edits.is_empty() {
            return Err(ApplyFailure::invalid(InvalidPlanCause::Empty, None));
        }
        if edits.len() > MAX_RECHECK_EDITS {
            return Err(ApplyFailure::invalid(InvalidPlanCause::TooManyEdits, None));
        }

        let ordered = ordered_unique_edits(edits);
        let (replacement_bytes, removed_bytes) = validate_unique_edits(original, &ordered)?;
        validate_layout(original, &ordered)?;
        validate_utf16(original, &ordered)?;

        let Some(base_len) = original.len().checked_sub(removed_bytes) else {
            return Err(ApplyFailure::invalid(InvalidPlanCause::Overlap, None));
        };
        let Some(output_len) = base_len.checked_add(replacement_bytes) else {
            return Err(ApplyFailure::invalid(
                InvalidPlanCause::SnapshotTooLarge,
                None,
            ));
        };
        if output_len > MAX_RECHECK_SNAPSHOT_BYTES {
            return Err(ApplyFailure::invalid(
                InvalidPlanCause::SnapshotTooLarge,
                None,
            ));
        }

        let mut output = String::with_capacity(output_len);
        let mut position = 0_usize;
        for (index, edit) in &ordered {
            let start = usize::try_from(edit.range.start())
                .map_err(|_| ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)))?;
            let end = usize::try_from(edit.range.end())
                .map_err(|_| ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)))?;
            let Some(prefix) = original.get(position..start) else {
                return Err(ApplyFailure::invalid(InvalidPlanCause::Range, Some(*index)));
            };
            output.push_str(prefix);
            output.push_str(edit.replacement);
            position = end;
        }
        let Some(tail) = original.get(position..) else {
            return Err(ApplyFailure::invalid(InvalidPlanCause::Range, None));
        };
        output.push_str(tail);
        Ok(AppliedSnapshot { text: output })
    }

    #[cfg(test)]
    mod tests;
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
