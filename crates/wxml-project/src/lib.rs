//! Mini Program project model.
//!
//! Discovers `app.json`, pages and components, resolves `usingComponents`
//! references and reads the applicable TypeScript project configuration.
//! Analysis and LSP code query this model instead of walking the filesystem
//! themselves.
//!
//! The first implementation lands in phase 2/3 of the plan; until then the
//! crate deliberately exposes no public items.
