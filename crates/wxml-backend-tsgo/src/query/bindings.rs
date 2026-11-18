//! Script-side binding and registration fact placeholders.
//!
//! `P4` discovers Page/Component/Behavior registrations, props and events from
//! script analysis. This module declares the internal fact DTOs; no script is
//! parsed here.

use wxml_types::CapabilityStatus;

use crate::input::ProjectKey;

/// Mini-program registration kinds resolved by script analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationKind {
    /// `Page(...)` registration.
    Page,
    /// `Component(...)` registration.
    Component,
    /// `Behavior(...)` registration.
    Behavior,
}

/// One registration fact placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationFact {
    /// Registration kind.
    pub kind: RegistrationKind,
    /// Exported or assigned name when one exists.
    pub name: String,
}

/// Binding facts request for one script document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingsQuery {
    /// Project the document belongs to.
    pub project: ProjectKey,
    /// Normalized document path.
    pub path: String,
}

/// Binding facts result placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingsResult {
    /// Registrations found in the script.
    pub registrations: Vec<RegistrationFact>,
    /// Completeness and availability of the analysis.
    pub status: CapabilityStatus,
}
