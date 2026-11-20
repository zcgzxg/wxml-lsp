//! Script-side binding and registration fact placeholders.
//!
//! `P4` discovers Page/Component/Behavior registrations, props and events from
//! script analysis. This module declares the internal fact DTOs; no script is
//! parsed here.

use wxml_types::CapabilityStatus;

use wxml_backend_tsgo::input::ProjectKey;

/// Mini-program registration kinds resolved by script analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegistrationKind {
    /// `Page(...)` registration.
    Page,
    /// `Component(...)` registration.
    Component,
    /// `Behavior(...)` registration.
    Behavior,
}

/// One registration fact placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistrationFact {
    /// Registration kind.
    pub(crate) kind: RegistrationKind,
    /// Exported or assigned name when one exists.
    pub(crate) name: String,
}

/// Binding facts request for one script document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingsQuery {
    /// Project the document belongs to.
    pub(crate) project: ProjectKey,
    /// Normalized document path.
    pub(crate) path: String,
}

/// Binding facts result placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingsResult {
    /// Registrations found in the script.
    pub(crate) registrations: Vec<RegistrationFact>,
    /// Completeness and availability of the analysis.
    pub(crate) status: CapabilityStatus,
}
