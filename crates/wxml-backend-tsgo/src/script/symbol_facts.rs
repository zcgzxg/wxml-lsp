//! Reusable script facts for the LSP symbol channels.
//!
//! [`SymbolFacts`] mirrors [`wxml_types::SymbolResult`] one-to-one but keeps
//! project-relative paths instead of `FileId`s. The adapter owns the view
//! lookup and calls [`SymbolFacts::into_symbol_result`]; this layer never opens
//! a native session, writes a file or fabricates a reference role.
//!
//! Completeness is conservative. A query is `complete` only when every fact it
//! uses came from the analyzed script, the registration and the relevant
//! overlay are complete, the scanners saw no dynamic escape and no array was
//! truncated. Anything else stays `Partial` with `complete = false`; the
//! proven subset is still published.

use std::collections::BTreeSet;

use wxml_types::{
    CapabilityStatus, FileId, MAX_SYMBOL_NAME_BYTES, MAX_SYMBOL_PATH, SymbolField,
    SymbolOccurrence, SymbolOccurrenceKind, SymbolReference, SymbolReferenceRole, SymbolResult,
    SymbolToken,
};

use crate::shadow::mapping::SourceRange;

use super::scanner::Script;
use super::symbols::ScriptRange;

pub use super::symbol_query::query_symbol_facts;

/// Bounded reason length used by `SymbolResult.reason`.
const MAX_REASON_BYTES: usize = 512;

/// One exact token in an original project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolTokenFact {
    /// Project-relative script path.
    pub path: String,
    /// Exact token range in both UTF-8 bytes and UTF-16 code units.
    pub range: ScriptRange,
    /// Exact original text of [`Self::range`].
    pub expected_text: Option<String>,
}

/// One script reference with its role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolReferenceFact {
    /// Exact reference token.
    pub token: SymbolTokenFact,
    /// Read, write or declaration role.
    pub role: SymbolReferenceRole,
}

/// One recognized `setData`/`observer` path occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolOccurrenceFact {
    /// Exact segment token, without quotes or dots.
    pub token: SymbolTokenFact,
    /// API that owns the occurrence.
    pub kind: SymbolOccurrenceKind,
    /// Complete statically written path.
    pub path: Vec<String>,
    /// Zero-based index of [`Self::token`] inside [`Self::path`].
    pub segment: usize,
}

/// One zero-width insertion anchor for a missing event handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerInsertionFact {
    /// Project-relative script receiving the insertion.
    pub path: String,
    /// UTF-8 byte offset of the closing brace.
    pub offset: u32,
    /// Preformatted member text.
    pub text: String,
}

/// One declared field below a data root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolFieldFact {
    /// Name as declared.
    pub name: String,
    /// Declaration token, when one can be named.
    pub declaration: Option<SymbolTokenFact>,
    /// True when the declaration was written as a string literal.
    pub quoted: bool,
}

/// Script facts for one symbol query.
///
/// This is the reusable channel shape for [`wxml_types::SymbolResult`]: every
/// token keeps a project-relative path and both coordinate systems, and the
/// status/complete/reason triple follows the same honesty rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolFacts {
    /// Availability and completeness of the query.
    pub status: CapabilityStatus,
    /// True only when every relevant occurrence is covered.
    pub complete: bool,
    /// Why the result is partial.
    pub reason: Option<String>,
    /// Declaration token of the queried symbol.
    pub declaration: Option<SymbolTokenFact>,
    /// Reference tokens, including the declaration.
    pub references: Vec<SymbolReferenceFact>,
    /// Recognized path occurrences.
    pub occurrences: Vec<SymbolOccurrenceFact>,
    /// Insertion anchor for a missing event handler.
    pub insertion: Option<HandlerInsertionFact>,
    /// Declared sibling fields.
    pub fields: Vec<SymbolFieldFact>,
    /// Names already reserved by the owner.
    pub reserved: Vec<String>,
}

impl SymbolFacts {
    /// Creates an unsupported answer with no fabricated facts.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            status: CapabilityStatus::Unsupported {
                reason: reason.into(),
            },
            complete: false,
            reason: None,
            declaration: None,
            references: Vec::new(),
            occurrences: Vec::new(),
            insertion: None,
            fields: Vec::new(),
            reserved: Vec::new(),
        }
    }

    /// Maps these facts to the shared DTO.
    ///
    /// `file_for` is the adapter's view lookup. A token whose path is not in
    /// the request view is dropped and makes the answer partial; ranges are
    /// never clamped.
    #[must_use]
    pub fn into_symbol_result<F>(self, mut file_for: F) -> SymbolResult
    where
        F: FnMut(&str) -> Option<FileId>,
    {
        let mut mapper = TokenMapper {
            file_for: &mut file_for,
            lost: false,
        };
        let declaration = self
            .declaration
            .as_ref()
            .and_then(|token| mapper.token(token));
        let references = self
            .references
            .iter()
            .filter_map(|reference| {
                mapper.token(&reference.token).map(|token| SymbolReference {
                    token,
                    role: reference.role,
                })
            })
            .collect();
        let occurrences = self
            .occurrences
            .iter()
            .filter_map(|occurrence| {
                mapper
                    .token(&occurrence.token)
                    .map(|token| SymbolOccurrence {
                        token,
                        kind: occurrence.kind,
                        path: occurrence.path.clone(),
                        segment: occurrence.segment,
                    })
            })
            .collect();
        let insertion = self
            .insertion
            .as_ref()
            .and_then(|insertion| mapper.insertion(insertion));
        let fields = self
            .fields
            .iter()
            .map(|field| SymbolField {
                name: field.name.clone(),
                declaration: field
                    .declaration
                    .as_ref()
                    .and_then(|token| mapper.token(token)),
                quoted: field.quoted,
            })
            .collect();
        let lost = mapper.lost;

        let mut complete = self.complete && self.status.is_supported() && !lost;
        let mut reasons = self.reason.into_iter().collect::<Vec<_>>();
        if lost {
            complete = false;
            reasons.push("some symbol facts could not be mapped to the request view".to_owned());
        }
        let reason = (!reasons.is_empty()).then(|| {
            bounded_reason(
                &reasons,
                "some symbol facts could not be mapped to the request view",
            )
        });
        let status = if self.status.is_supported() && complete {
            CapabilityStatus::Supported
        } else if self.status.is_supported() {
            CapabilityStatus::Partial {
                reason: reason
                    .clone()
                    .unwrap_or_else(|| "script symbol coverage is partial".to_owned()),
            }
        } else {
            complete = false;
            self.status.clone()
        };
        SymbolResult {
            status,
            complete,
            reason,
            declaration,
            references,
            occurrences,
            insertion: insertion.map(|insertion| wxml_types::HandlerInsertion {
                file: insertion.file,
                offset: insertion.offset,
                text: insertion.text,
            }),
            fields,
            reserved: self.reserved,
        }
    }
}

struct MappedInsertion {
    file: FileId,
    offset: u32,
    text: String,
}

struct TokenMapper<'a, F> {
    file_for: &'a mut F,
    lost: bool,
}

impl<F> TokenMapper<'_, F>
where
    F: FnMut(&str) -> Option<FileId>,
{
    fn token(&mut self, fact: &SymbolTokenFact) -> Option<SymbolToken> {
        let Some(file) = (self.file_for)(&fact.path) else {
            self.lost = true;
            return None;
        };
        let Ok(range) = wxml_types::SourceRange::from_bounds(
            fact.range.utf8.start.get(),
            fact.range.utf8.end.get(),
        ) else {
            self.lost = true;
            return None;
        };
        Some(SymbolToken {
            file,
            range,
            expected_text: fact.expected_text.clone(),
        })
    }

    fn insertion(&mut self, fact: &HandlerInsertionFact) -> Option<MappedInsertion> {
        let Some(file) = (self.file_for)(&fact.path) else {
            self.lost = true;
            return None;
        };
        Some(MappedInsertion {
            file,
            offset: fact.offset,
            text: fact.text.clone(),
        })
    }
}

pub(crate) fn finish(
    complete: bool,
    mut problems: Vec<String>,
    mut facts: SymbolFacts,
) -> SymbolFacts {
    dedupe(&mut problems);
    if facts.status.is_supported() && complete {
        facts.status = CapabilityStatus::Supported;
        facts.complete = true;
        facts.reason = None;
        return facts;
    }
    let reason = bounded_reason(&problems, "script symbol coverage is partial");
    facts.status = CapabilityStatus::Partial {
        reason: reason.clone(),
    };
    facts.complete = false;
    facts.reason = Some(reason);
    facts
}

pub(crate) fn validate_data_path(path: &[String]) -> Result<(), String> {
    if path.is_empty() {
        return Err("the data path is empty".to_owned());
    }
    if path.len() > MAX_SYMBOL_PATH {
        return Err(format!("the data path exceeds {MAX_SYMBOL_PATH} segments"));
    }
    for segment in path {
        if segment.is_empty() {
            return Err("the data path contains an empty segment".to_owned());
        }
        if segment.len() > MAX_SYMBOL_NAME_BYTES {
            return Err("a data path segment is too long".to_owned());
        }
        if segment
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
            || segment.contains(['.', '[', ']'])
        {
            return Err("the data path contains an unsupported character".to_owned());
        }
    }
    Ok(())
}

/// Builds a token fact for a raw key token.
///
/// Quoted keys return the content range without the quotes. `escaped` is true
/// when the content contains a backslash, because a rename would otherwise
/// have to re-escape the new text.
pub(crate) fn key_token(
    script: &Script<'_>,
    path: &str,
    key_range: SourceRange,
) -> Option<(SymbolTokenFact, bool, bool)> {
    let raw = range_text(script.text, key_range)?;
    if raw.starts_with('"') || raw.starts_with('\'') {
        let quote = raw.chars().next()?;
        if raw.len() < 2 || !raw.ends_with(quote) {
            return None;
        }
        let content = SourceRange::new(key_range.start.get() + 1, key_range.end.get() - 1);
        let expected = range_text(script.text, content)?;
        Some((
            SymbolTokenFact {
                path: path.to_owned(),
                range: script.source_range(content).ok()?,
                expected_text: Some(expected.to_owned()),
            },
            true,
            expected.contains('\\'),
        ))
    } else {
        Some((
            SymbolTokenFact {
                path: path.to_owned(),
                range: script.source_range(key_range).ok()?,
                expected_text: Some(raw.to_owned()),
            },
            false,
            false,
        ))
    }
}

/// Returns the exact text of one byte range.
pub(crate) fn range_text(text: &str, range: SourceRange) -> Option<&str> {
    text.get(range.start.get() as usize..range.end.get() as usize)
}

pub(crate) fn push_reason(reasons: &mut Vec<String>, reason: &str) {
    if !reasons.iter().any(|existing| existing == reason) {
        reasons.push(reason.to_owned());
    }
}

pub(crate) fn dedupe(reasons: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    reasons.retain(|reason| seen.insert(reason.clone()));
}

pub(crate) fn bounded_reason(reasons: &[String], fallback: &str) -> String {
    let joined = if reasons.is_empty() {
        fallback.to_owned()
    } else {
        reasons.join("; ")
    };
    if joined.len() <= MAX_REASON_BYTES {
        return joined;
    }
    let mut end = MAX_REASON_BYTES;
    while end > 0 && !joined.is_char_boundary(end) {
        end -= 1;
    }
    joined[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use wxml_types::ProjectId;

    use super::*;

    fn token(path: &str, start: u32, end: u32) -> SymbolTokenFact {
        SymbolTokenFact {
            path: path.to_owned(),
            range: ScriptRange {
                utf8: SourceRange::new(start, end),
                utf16: super::super::symbols::Utf16Range::new(start, end),
            },
            expected_text: Some("name".to_owned()),
        }
    }

    fn facts() -> SymbolFacts {
        SymbolFacts {
            status: CapabilityStatus::Supported,
            complete: true,
            reason: None,
            declaration: Some(token("index.ts", 10, 14)),
            references: vec![SymbolReferenceFact {
                token: token("index.ts", 10, 14),
                role: SymbolReferenceRole::Declaration,
            }],
            occurrences: vec![SymbolOccurrenceFact {
                token: token("index.ts", 20, 24),
                kind: SymbolOccurrenceKind::SetDataKey,
                path: vec!["user".to_owned(), "name".to_owned()],
                segment: 1,
            }],
            insertion: Some(HandlerInsertionFact {
                path: "index.ts".to_owned(),
                offset: 30,
                text: "onSave() {}".to_owned(),
            }),
            fields: vec![SymbolFieldFact {
                name: "name".to_owned(),
                declaration: Some(token("index.ts", 10, 14)),
                quoted: false,
            }],
            reserved: vec!["onTap".to_owned()],
        }
    }

    #[test]
    fn maps_every_channel_when_the_view_owns_the_files() {
        let project = ProjectId::new("demo");
        let result = facts().into_symbol_result(|path| FileId::new(project.clone(), path).ok());
        assert!(result.complete);
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert!(result.declaration.is_some());
        assert_eq!(result.references.len(), 1);
        assert_eq!(result.occurrences.len(), 1);
        assert_eq!(result.occurrences[0].segment, 1);
        assert!(result.insertion.is_some());
        assert_eq!(result.fields.len(), 1);
        assert_eq!(result.reserved, vec!["onTap".to_owned()]);
    }

    #[test]
    fn unmapped_paths_drop_tokens_and_force_partial() {
        let result = facts().into_symbol_result(|_| None);
        assert!(!result.complete);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
        assert!(result.reason.is_some());
        assert!(result.declaration.is_none());
        assert_eq!(result.references.len(), 0);
        assert_eq!(result.fields.len(), 1);
        assert!(result.fields[0].declaration.is_none());
    }

    #[test]
    fn unsupported_facts_never_claim_completeness() {
        let result = SymbolFacts::unsupported("no registration").into_symbol_result(|_| None);
        assert!(!result.complete);
        assert!(matches!(
            result.status,
            CapabilityStatus::Unsupported { .. }
        ));
    }

    #[test]
    fn reasons_are_bounded_on_a_character_boundary() {
        let reasons = vec!["中".repeat(300), "x".repeat(300)];
        let reason = bounded_reason(&reasons, "fallback");
        assert!(reason.len() <= MAX_REASON_BYTES);
        assert!(reason.is_char_boundary(reason.len()));
    }
}
