//! WXML syntax: lexing, parsing and template expressions.
//!
//! This crate turns WXML source into an edit-friendly syntax tree with byte
//! ranges, including `{{ ... }}` mustache expressions. It knows nothing about
//! editors or TypeScript types.
//!
//! Planned modules: `lexer`, `parser`, `ast`, `expr` and `source_map`. The
//! first parser implementation lands in implementation phase 2; until then
//! the crate deliberately exposes no public items.
