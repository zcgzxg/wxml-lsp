//! Classification shared by static and Node-backed completion.

use wxml_project::ProjectPath;
use wxml_types::CompletionKind;

use super::context::{Context, Site};

/// Where a completion site is answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Route {
    /// Answered synchronously from the static catalog.
    Static,
    /// Requires the type backend (Node) and a prepared view.
    Backend(CompletionKind),
}

/// Classifies one detected site.
pub(crate) fn route(context: &Context) -> Route {
    match &context.site {
        Site::Tag { .. } | Site::AttributeName { .. } => Route::Static,
        Site::AttributeValue { attribute, .. } => {
            if super::catalog::is_event_prefix(attribute) {
                Route::Backend(CompletionKind::EventHandler)
            } else {
                Route::Static
            }
        }
        Site::Mustache => {
            // Only the actual expression is a member query; `{{ }}` around a
            // literal would be filtered by the handler before scheduling.
            Route::Backend(CompletionKind::ExpressionMember)
        }
    }
}

/// Returns the project path of a WXML file, when it is a project document.
pub(crate) fn project_path(path: &str) -> Option<ProjectPath> {
    ProjectPath::new(path).ok()
}
