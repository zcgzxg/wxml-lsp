//! Script symbol facts for the native adapter.
//!
//! `Backend::symbols` answers data-root, data-path and event-handler queries
//! from the Rust script overlay. This path deliberately does not open a native
//! session, build a shadow or call the TypeScript checker: the reusable facts
//! live in [`wxml_backend_tsrs::script::symbol_facts`] and are derived from the
//! same [`wxml_backend_tsrs::script::ScriptAnalysis`] used by event-handler
//! completion.
//!
//! The prepared view is the only project snapshot. Every reference token is
//! mapped back through the view's exact `PreparedFile` text, so a non-owner
//! overlay is visible without restarting a session. A missing subject, an
//! incomplete source list, a truncated fact list or any unmappable token makes
//! the answer `Partial`; it never becomes a supported empty result.

use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsrs::script::registration::RegistrationKind;
use wxml_backend_tsrs::script::symbol_facts::query_symbol_facts;
use wxml_backend_tsrs::script::{ScriptRequest, analyze_script};
use wxml_types::{BackendError, CapabilityStatus, RootKind, SymbolRequest, SymbolResult};

use crate::backend_tsrs::ViewProvider;
use crate::backend_tsrs::input::prepare_view;
use crate::project_host::PreparedView;

/// Stable reason for a template without a page or component owner.
const NO_TARGET: &str = "the template has no page or component script";
/// Stable reason for a subject outside the prepared view.
const SUBJECT_NOT_IN_VIEW: &str = "the symbol's script is not part of the prepared view";
/// Stable reason for a script whose syntactic facts cannot be scanned.
const SCRIPT_UNAVAILABLE: &str = "script symbol facts are unavailable for this script";
/// Stable reason for an incomplete prepared source list.
const SOURCE_INCOMPLETE: &str = "the prepared source file list is incomplete";

/// Answers one script symbol query from the prepared view.
///
/// This is the adapter seam next to [`crate::backend_tsrs::event_completion`]:
/// it only asks the [`ViewProvider`] for the effective project files and never
/// starts the native session. The caller owns the epoch check.
pub(crate) fn symbols(
    provider: &dyn ViewProvider,
    request: &SymbolRequest,
) -> Result<SymbolResult, BackendError> {
    let view = prepare_view(provider, request.context.source())?;
    let Some(target) = view.target.as_ref() else {
        return Ok(SymbolResult::unsupported(NO_TARGET));
    };
    let Some(subject) = view.files.iter().find(|file| file.file == request.subject) else {
        return Ok(SymbolResult::unsupported(SUBJECT_NOT_IN_VIEW));
    };
    let subject_path = subject.path.as_str().to_owned();
    let kind = match target.owner {
        RootKind::Page => RegistrationKind::Page,
        RootKind::Component => RegistrationKind::Component,
    };

    let Some(root) = view.project_root.to_str() else {
        return Err(BackendError::Unavailable(
            "the project root is not valid UTF-8".to_owned(),
        ));
    };
    let project_root = ProjectRoot::new(root.to_owned())
        .map_err(|_| BackendError::Unavailable("the project root is invalid".to_owned()))?;

    let mut files = Vec::with_capacity(view.files.len());
    let mut dropped = 0_usize;
    for prepared in view.files.iter() {
        let represented = SourceFile::new(
            prepared.path.as_str(),
            prepared.text.to_string(),
            DocumentRevision::new(prepared.source_revision),
        );
        match represented {
            Ok(file) => files.push(file),
            Err(_) if prepared.path.as_str() == subject_path => {
                return Ok(SymbolResult::unsupported(SCRIPT_UNAVAILABLE));
            }
            Err(_) => dropped = dropped.saturating_add(1),
        }
    }

    let input = AnalysisInput::new(
        ProjectKey::new(request.context.file().project().as_str()),
        project_root,
        SnapshotRevision::new(1),
        files,
    )
    .map_err(|_| BackendError::Unavailable("the symbol request input is invalid".to_owned()))?;

    let Ok(analysis) = analyze_script(&ScriptRequest::new(&input, &subject_path, kind)) else {
        return Ok(SymbolResult::unsupported(SCRIPT_UNAVAILABLE));
    };
    let facts = query_symbol_facts(&analysis, &input, &request.query);
    let result = facts.into_symbol_result(|path| {
        view.files
            .iter()
            .find(|file| file.path.as_str() == path)
            .map(|file| file.file.clone())
    });
    Ok(degrade(result, &view, dropped))
}

/// Marks a result partial when the prepared view or file conversion lost data.
fn degrade(mut result: SymbolResult, view: &PreparedView, dropped: usize) -> SymbolResult {
    let mut reasons = Vec::new();
    if !view.source_complete {
        reasons.push(SOURCE_INCOMPLETE.to_owned());
    }
    if dropped > 0 {
        reasons.push(format!(
            "{dropped} script file(s) could not be represented in the symbol snapshot"
        ));
    }
    if reasons.is_empty() {
        return result;
    }
    let reason = bounded_reason(&reasons.join("; "));
    result.complete = false;
    result.status = match result.status {
        CapabilityStatus::Supported => CapabilityStatus::Partial {
            reason: reason.clone(),
        },
        CapabilityStatus::Partial { reason: existing } => CapabilityStatus::Partial {
            reason: append_reason(&existing, &reason),
        },
        other => other,
    };
    result.reason = Some(match result.reason {
        Some(existing) => append_reason(&existing, &reason),
        None => reason,
    });
    result
}

/// Appends one reason unless the exact text is already present.
fn append_reason(existing: &str, reason: &str) -> String {
    if existing.split("; ").any(|part| part == reason) {
        existing.to_owned()
    } else {
        bounded_reason(&format!("{existing}; {reason}"))
    }
}

/// Caps a reason at [`crate::backend_tsrs::MAX_REASON_BYTES`] on a character boundary.
fn bounded_reason(reason: &str) -> String {
    if reason.len() <= crate::backend_tsrs::MAX_REASON_BYTES {
        return reason.to_owned();
    }
    let mut end = crate::backend_tsrs::MAX_REASON_BYTES;
    while end > 0 && !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_owned()
}
