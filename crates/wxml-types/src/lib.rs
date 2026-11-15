//! Template type analysis with a pluggable backend.
//!
//! Converts parsed templates plus project context into an in-memory check
//! view, asks a type backend for hovers and diagnostics, and maps the results
//! back to WXML byte ranges. The backend trait keeps the Node + TypeScript
//! Language Service implementation replaceable.
//!
//! The first check-view prototype lands in phase 2 of the plan; until then
//! the crate deliberately exposes no public items.
