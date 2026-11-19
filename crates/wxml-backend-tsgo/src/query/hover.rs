//! Hover facts and Markdown composition.
//!
//! The native engine has no `getQuickInfoAtPosition`. A hover answer is a
//! composition of independent facts: the type at the position, the symbol and
//! its declarations, and the symbol's documentation and `JSDoc` tags. This
//! module keeps those facts separate so callers can inspect what was actually
//! proven instead of receiving a pre-rendered QuickInfo-like string.
//!
//! [`compose_hover_markdown`] is pure and follows the frozen three-part order:
//! an exact ` ```ts ` type fence, the documentation comment verbatim, and one
//! `@name text` line per tag. There is no `QuickInfo` prefix, no overload
//! segmentation and no fabricated symbol or alias label.
//!
//! An empty Markdown string is never paired with
//! [`CapabilityStatus::Supported`]: a checked-empty documentation query stays
//! visible as the documentation status, while a hover with no usable type or
//! documentation is [`CapabilityStatus::Unsupported`].
//!
//! Only [`HoverSymbolFacts::declarations`] carry epoch bindings. The type,
//! quality, documentation and decoded declaration values are plain query
//! facts; the session re-checks its epoch/content identity before returning,
//! but the DTO itself does not bind those values to an epoch. The hover token
//! range is intentionally absent: selecting it from the request is the
//! adapter's responsibility, and this module never fabricates a range.

use wxml_types::CapabilityStatus;

use crate::native::handles::BoundNodeHandle;
use crate::query::documentation::DocumentationResult;
use crate::query::type_info::{TypeInfo, TypeQuality};
use crate::script::symbols::Utf16Range;
use crate::shadow::mapping::SourceRange;

/// Maximum number of distinct declaration targets retained for one hover.
///
/// This matches the native definition target cap used by this crate (`16`).
/// It is not the Node adapter's separate publication cap. Deduplication by
/// `(path, start, end)` happens before the cap is applied.
pub const MAX_HOVER_DECLARATIONS: usize = 16;

/// One declaration target in original coordinates.
///
/// The path is normalized and project-relative. Both coordinate systems are
/// carried because the AST produces UTF-16 and the C0 boundary consumes UTF-8;
/// the pair comes from the same source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverDeclaration {
    /// Normalized project-relative target path.
    pub path: String,
    /// Original UTF-8 byte range of the declaration's name token.
    pub range: SourceRange,
    /// The same token range in UTF-16 code units.
    pub utf16: Utf16Range,
}

/// Symbol facts retained for one hover.
///
/// The declarations are epoch-bound handles; [`HoverEvidence::declarations`]
/// carries the verified project-relative targets decoded from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverSymbolFacts {
    /// Symbol name exactly as reported by the engine.
    pub name: String,
    /// Native symbol flags.
    pub flags: u64,
    /// Epoch-bound declaration handles.
    pub declarations: Vec<BoundNodeHandle>,
}

/// A typed reason why the hover evidence is incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoverGap {
    /// A required native capability could not answer.
    Unsupported {
        /// Human-readable, non-fabricated explanation.
        reason: String,
    },
    /// A declaration is outside the project snapshot, such as a
    /// standard-library or mounted type-asset declaration.
    ExternalDeclaration {
        /// Path reported by the engine when it can be named; this may be an
        /// absolute asset path rather than a project-relative path. `None`
        /// keeps the degradation but uses a neutral reason rather than
        /// claiming a path that was not proven.
        path: Option<String>,
    },
    /// A declaration handle could not be decoded into a verified source token.
    UndecodableDeclaration {
        /// Human-readable, non-fabricated explanation.
        reason: String,
    },
    /// A declaration was decoded far enough to see that its name token does
    /// not match the queried symbol, for example an alias target whose own
    /// declaration name differs from the alias.
    NameMismatchDeclaration {
        /// Queried symbol name the declaration name did not match.
        expected: String,
    },
    /// The type is `Missing`, but symbol or declaration evidence cannot prove
    /// that the position truly has no type. The existing quality is kept
    /// instead of guessing a different one.
    Blocked {
        /// Human-readable, non-fabricated explanation.
        reason: String,
    },
    /// The declaration list was cut at [`MAX_HOVER_DECLARATIONS`].
    TruncatedDeclarations {
        /// Applied cap.
        limit: usize,
    },
    /// Decoding was stopped after the fixed declaration scan cap.
    TruncatedDeclarationScan {
        /// Applied scan cap.
        limit: usize,
    },
}

impl HoverGap {
    fn reason(&self) -> String {
        match self {
            Self::Unsupported { reason }
            | Self::UndecodableDeclaration { reason }
            | Self::Blocked { reason } => reason.clone(),
            Self::ExternalDeclaration { path } => path.as_ref().map_or_else(
                || "a declaration could not be bound to the project snapshot".to_owned(),
                |path| format!("declaration `{path}` is outside the project snapshot"),
            ),
            Self::NameMismatchDeclaration { expected } => {
                format!("a declaration name does not match symbol `{expected}`")
            }
            Self::TruncatedDeclarations { limit } => {
                format!("declaration list was truncated at {limit} entries")
            }
            Self::TruncatedDeclarationScan { limit } => {
                format!("declaration scan was truncated at {limit} handles")
            }
        }
    }
}

/// Outcome of inserting one declaration target into the bounded set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclarationInsert {
    /// The target was new and has been stored.
    Inserted,
    /// An identical `(path, start, end)` target was already stored.
    Duplicate,
    /// The set already holds [`MAX_HOVER_DECLARATIONS`] targets.
    Truncated,
}

/// Deduplicates and caps declaration targets in engine order.
pub(crate) fn insert_declaration(
    declarations: &mut Vec<HoverDeclaration>,
    target: HoverDeclaration,
) -> DeclarationInsert {
    if declarations
        .iter()
        .any(|existing| existing.path == target.path && existing.range == target.range)
    {
        return DeclarationInsert::Duplicate;
    }
    if declarations.len() >= MAX_HOVER_DECLARATIONS {
        return DeclarationInsert::Truncated;
    }
    declarations.push(target);
    DeclarationInsert::Inserted
}

/// Truthful hover evidence for one position.
///
/// An empty [`HoverEvidence::declarations`] list means "checked, no decodable
/// declaration" only when the corresponding symbol facts and gaps say so. An
/// absent [`HoverEvidence::documentation`] means no documentation query was
/// applicable (for example a symbol without a content-addressed reference); a
/// present documentation result with no comment and no tags is a checked-empty
/// answer, distinct from a documentation call that failed.
///
/// Only [`HoverSymbolFacts::declarations`] are epoch-bound handles. The type,
/// quality and documentation values are plain facts from this query; the DTO
/// carries no separate epoch binding for them. The request's hover token range
/// is intentionally not part of this evidence: the adapter owns mapping the
/// request through the shadow layout, and the backend never fabricates one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverEvidence {
    /// Type facts when the engine returned a type object.
    pub type_info: Option<TypeInfo>,
    /// Type quality, including `Missing` for a successful "no type" answer.
    pub type_quality: TypeQuality,
    /// Symbol facts, when a symbol exists at the position.
    pub symbol: Option<HoverSymbolFacts>,
    /// Documentation facts, when a documentation query was applicable.
    pub documentation: Option<DocumentationResult>,
    /// Verified project-relative declaration targets.
    pub declarations: Vec<HoverDeclaration>,
    /// Completeness and availability of the composed answer.
    pub status: CapabilityStatus,
    /// Typed reasons why the evidence is incomplete.
    pub gaps: Vec<HoverGap>,
}

impl HoverEvidence {
    /// Recomputes the status from the facts and typed gaps.
    ///
    /// A `Missing` type with symbol or declaration evidence is recorded as
    /// [`HoverGap::Blocked`]; the quality itself is never rewritten.
    ///
    /// The composed hover is `Unsupported` when no type or documentation can be
    /// rendered, even if every individual query succeeded. A non-empty answer
    /// is `Partial` as soon as any fact is unknown, missing, external,
    /// undecodable or truncated, and `Supported` only when every usable fact is
    /// complete.
    pub fn finalize_status(&mut self) {
        let mut unique_gaps = Vec::with_capacity(self.gaps.len());
        for gap in self.gaps.drain(..) {
            if !unique_gaps.contains(&gap) {
                unique_gaps.push(gap);
            }
        }
        self.gaps = unique_gaps;
        if self.type_quality == TypeQuality::Missing
            && (self.symbol.is_some() || !self.declarations.is_empty())
            && !self
                .gaps
                .iter()
                .any(|gap| matches!(gap, HoverGap::Blocked { .. }))
        {
            self.gaps.push(HoverGap::Blocked {
                reason: "the type is Missing but symbol or declaration evidence exists".to_owned(),
            });
        }

        let has_content = self.has_renderable_content();
        let mut reasons = self.degradation_reasons(has_content);
        reasons.sort();
        reasons.dedup();

        self.status = if !has_content {
            let reason = if reasons.is_empty() {
                "no type or documentation evidence is available at the position".to_owned()
            } else {
                reasons.join("; ")
            };
            CapabilityStatus::Unsupported { reason }
        } else if reasons.is_empty() {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Partial {
                reason: reasons.join("; "),
            }
        };
    }

    fn has_renderable_content(&self) -> bool {
        self.renderable_type().is_some() || self.renderable_documentation()
    }

    fn renderable_type(&self) -> Option<&str> {
        let info = self.type_info.as_ref()?;
        if info.text.is_empty() {
            return None;
        }
        match self.type_quality {
            TypeQuality::Known => Some(info.text.as_str()),
            TypeQuality::Any if info.text == "any" => Some(info.text.as_str()),
            TypeQuality::Any | TypeQuality::Unknown | TypeQuality::Missing => None,
        }
    }

    fn renderable_documentation(&self) -> bool {
        self.documentation.as_ref().is_some_and(|documentation| {
            documentation
                .comment
                .as_deref()
                .is_some_and(|comment| !comment.trim().is_empty())
                || !documentation.tags.is_empty()
        })
    }

    fn degradation_reasons(&self, has_content: bool) -> Vec<String> {
        let mut reasons = Vec::new();
        if has_content {
            match self.type_quality {
                TypeQuality::Missing => {
                    reasons.push("the hover type could not be established".to_owned());
                }
                TypeQuality::Unknown => {
                    reasons.push("the hover type is unknown".to_owned());
                }
                TypeQuality::Any | TypeQuality::Known => {}
            }
        }
        if let Some(docs) = &self.documentation {
            if let Some(reason) = docs.status.reason() {
                reasons.push(format!("documentation: {reason}"));
            }
        }
        reasons.extend(self.gaps.iter().map(HoverGap::reason));
        reasons
    }
}

/// Composes the frozen three-part Markdown answer.
///
/// The parts are the exact ` ```ts ` type fence, the documentation comment
/// verbatim, and one `@name text` line per tag, in that order and separated by
/// one blank line. The fence grows only when the type text itself contains a
/// backtick run, so the same verbatim block cannot be closed early. There is no
/// `QuickInfo` prefix, no per-overload segmentation and no symbol or alias
/// label. When no part is renderable, the answer is the empty string; callers
/// must use [`HoverEvidence::status`] to distinguish that `Unsupported` case
/// from a checked-empty fact.
#[must_use]
pub fn compose_hover_markdown(evidence: &HoverEvidence) -> String {
    let mut sections = Vec::new();
    if let Some(text) = evidence.renderable_type() {
        sections.push(type_fence(text));
    }
    if let Some(documentation) = &evidence.documentation {
        if let Some(comment) = documentation
            .comment
            .as_deref()
            .filter(|comment| !comment.trim().is_empty())
        {
            sections.push(comment.to_owned());
        }
        if !documentation.tags.is_empty() {
            let tags = documentation
                .tags
                .iter()
                .map(|tag| {
                    tag.text
                        .as_deref()
                        .filter(|text| !text.trim().is_empty())
                        .map_or_else(
                            || format!("@{}", tag.name),
                            |text| format!("@{} {text}", tag.name),
                        )
                })
                .collect::<Vec<_>>();
            sections.push(tags.join("\n"));
        }
    }
    sections.join("\n\n")
}

/// Renders a type in a `ts` fence long enough not to be broken by the text.
///
/// Triple backticks are used for every normal type text, including overloads.
/// If the engine text itself contains a backtick run, the fence grows by one
/// backtick so the type remains one verbatim block instead of being escaped or
/// truncated.
fn type_fence(text: &str) -> String {
    let fence = "`".repeat(longest_backtick_run(text).saturating_add(1).max(3));
    format!("{fence}ts\n{text}\n{fence}")
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for character in text.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::query::documentation::JsDocTagInfo;

    fn info(text: &str) -> TypeInfo {
        TypeInfo {
            text: text.to_owned(),
            handle: 1,
            flags: 1,
        }
    }

    fn docs(
        comment: Option<&str>,
        tags: Vec<JsDocTagInfo>,
        status: CapabilityStatus,
    ) -> DocumentationResult {
        DocumentationResult {
            comment: comment.map(str::to_owned),
            tags,
            status,
        }
    }

    fn evidence(
        info: Option<TypeInfo>,
        quality: TypeQuality,
        documentation: Option<DocumentationResult>,
    ) -> HoverEvidence {
        HoverEvidence {
            type_info: info,
            type_quality: quality,
            symbol: None,
            documentation,
            declarations: Vec::new(),
            status: CapabilityStatus::Supported,
            gaps: Vec::new(),
        }
    }

    #[test]
    fn composition_is_exact_and_ordered() {
        let mut hover = evidence(
            Some(info("string")),
            TypeQuality::Known,
            Some(docs(
                Some("A user."),
                vec![JsDocTagInfo {
                    name: "deprecated".to_owned(),
                    text: Some("use Person".to_owned()),
                }],
                CapabilityStatus::Supported,
            )),
        );
        hover.finalize_status();
        assert_eq!(
            compose_hover_markdown(&hover),
            "```ts\nstring\n```\n\nA user.\n\n@deprecated use Person"
        );
    }

    #[test]
    fn unknown_is_not_wrapped_and_any_keeps_its_literal_text() {
        let mut unknown = evidence(Some(info("unknown")), TypeQuality::Unknown, None);
        unknown.finalize_status();
        assert_eq!(compose_hover_markdown(&unknown), "");
        assert!(matches!(
            unknown.status,
            CapabilityStatus::Unsupported { .. }
        ));

        let mut any = evidence(Some(info("any")), TypeQuality::Any, None);
        any.finalize_status();
        assert_eq!(compose_hover_markdown(&any), "```ts\nany\n```");
        assert_eq!(any.status, CapabilityStatus::Supported);
    }

    #[test]
    fn missing_with_symbol_evidence_is_blocked_without_rewriting_quality() {
        let mut hover = evidence(None, TypeQuality::Missing, None);
        hover.symbol = Some(HoverSymbolFacts {
            name: "u".to_owned(),
            flags: 1,
            declarations: Vec::new(),
        });
        hover.finalize_status();
        assert!(
            hover
                .gaps
                .iter()
                .any(|gap| matches!(gap, HoverGap::Blocked { .. }))
        );
        assert_eq!(hover.type_quality, TypeQuality::Missing);
    }

    #[test]
    fn empty_facts_are_unsupported_and_typed_gaps_degrade() {
        let mut empty = evidence(None, TypeQuality::Missing, None);
        empty.finalize_status();
        assert!(matches!(empty.status, CapabilityStatus::Unsupported { .. }));

        let mut external = evidence(Some(info("number")), TypeQuality::Known, None);
        external
            .gaps
            .push(HoverGap::ExternalDeclaration { path: None });
        external.finalize_status();
        assert!(matches!(external.status, CapabilityStatus::Partial { .. }));
        assert_eq!(compose_hover_markdown(&external), "```ts\nnumber\n```");
    }

    #[test]
    fn checked_empty_documentation_differs_from_unavailable_documentation() {
        let mut checked = evidence(
            Some(info("string")),
            TypeQuality::Known,
            Some(docs(None, Vec::new(), CapabilityStatus::Supported)),
        );
        checked.finalize_status();
        assert_eq!(checked.status, CapabilityStatus::Supported);

        let mut whitespace = evidence(
            Some(info("string")),
            TypeQuality::Known,
            Some(docs(
                Some("  \n\t"),
                Vec::new(),
                CapabilityStatus::Supported,
            )),
        );
        whitespace.finalize_status();
        assert_eq!(whitespace.status, CapabilityStatus::Supported);
        assert_eq!(compose_hover_markdown(&whitespace), "```ts\nstring\n```");

        let mut blank_only = evidence(
            None,
            TypeQuality::Missing,
            Some(docs(
                Some("  \n\t"),
                Vec::new(),
                CapabilityStatus::Supported,
            )),
        );
        blank_only.finalize_status();
        assert!(matches!(
            blank_only.status,
            CapabilityStatus::Unsupported { .. }
        ));
        assert_eq!(compose_hover_markdown(&blank_only), "");

        let mut unavailable = evidence(
            Some(info("string")),
            TypeQuality::Known,
            Some(docs(
                None,
                Vec::new(),
                CapabilityStatus::Unsupported {
                    reason: "documentation methods are unavailable".to_owned(),
                },
            )),
        );
        unavailable.finalize_status();
        assert!(matches!(
            unavailable.status,
            CapabilityStatus::Partial { .. }
        ));
    }

    #[test]
    fn truncation_and_tag_shapes_are_not_hidden() {
        let mut hover = evidence(
            Some(info("User")),
            TypeQuality::Known,
            Some(docs(
                None,
                vec![
                    JsDocTagInfo {
                        name: "since".to_owned(),
                        text: Some("1.0".to_owned()),
                    },
                    JsDocTagInfo {
                        name: "deprecated".to_owned(),
                        text: None,
                    },
                ],
                CapabilityStatus::Supported,
            )),
        );
        hover
            .gaps
            .push(HoverGap::TruncatedDeclarations { limit: 16 });
        hover.finalize_status();
        assert!(matches!(hover.status, CapabilityStatus::Partial { .. }));
        assert_eq!(
            compose_hover_markdown(&hover),
            "```ts\nUser\n```\n\n@since 1.0\n@deprecated"
        );
    }

    #[test]
    fn duplicate_reasons_are_reported_once() {
        let mut hover = evidence(Some(info("User")), TypeQuality::Known, None);
        hover
            .gaps
            .push(HoverGap::ExternalDeclaration { path: None });
        hover
            .gaps
            .push(HoverGap::ExternalDeclaration { path: None });
        hover.finalize_status();
        assert_eq!(hover.gaps.len(), 1);
        let CapabilityStatus::Partial { reason } = &hover.status else {
            panic!("expected partial");
        };
        assert_eq!(
            reason
                .matches("could not be bound to the project snapshot")
                .count(),
            1
        );
    }

    #[test]
    fn composition_does_not_use_symbol_facts_as_labels() {
        let mut hover = evidence(Some(info("User")), TypeQuality::Known, None);
        hover.symbol = Some(HoverSymbolFacts {
            name: "AliasName".to_owned(),
            flags: 2,
            declarations: Vec::new(),
        });
        hover.finalize_status();
        let markdown = compose_hover_markdown(&hover);
        assert_eq!(markdown, "```ts\nUser\n```");
        assert!(!markdown.contains("AliasName"));
    }

    #[test]
    fn fence_breaking_type_text_grows_the_block_instead_of_escaping() {
        let mut hover = evidence(Some(info("A | ``` | B")), TypeQuality::Known, None);
        hover.finalize_status();
        assert_eq!(compose_hover_markdown(&hover), "````ts\nA | ``` | B\n````");
        assert!(!compose_hover_markdown(&hover).contains("\n```\n"));
    }

    #[test]
    fn every_typed_gap_has_a_reason() {
        let gaps = [
            HoverGap::Unsupported {
                reason: "type queries are unavailable".to_owned(),
            },
            HoverGap::ExternalDeclaration {
                path: Some("model.ts".to_owned()),
            },
            HoverGap::ExternalDeclaration { path: None },
            HoverGap::UndecodableDeclaration {
                reason: "missing AST node".to_owned(),
            },
            HoverGap::NameMismatchDeclaration {
                expected: "Person".to_owned(),
            },
            HoverGap::Blocked {
                reason: "cannot prove Missing".to_owned(),
            },
            HoverGap::TruncatedDeclarations { limit: 16 },
            HoverGap::TruncatedDeclarationScan { limit: 64 },
        ];
        let mut unique = BTreeSet::new();
        for gap in &gaps {
            let reason = gap.reason();
            assert!(!reason.is_empty(), "{gap:?}");
            unique.insert(reason);
        }
        assert!(unique.len() >= gaps.len() - 1);
    }

    fn declaration(start: u32) -> HoverDeclaration {
        HoverDeclaration {
            path: "model.ts".to_owned(),
            range: SourceRange::new(start, start + 4),
            utf16: Utf16Range::new(start, start + 4),
        }
    }

    #[test]
    fn declaration_targets_dedupe_by_path_and_range_before_the_cap() {
        let mut declarations = Vec::new();
        assert_eq!(
            insert_declaration(&mut declarations, declaration(0)),
            DeclarationInsert::Inserted
        );
        assert_eq!(
            insert_declaration(&mut declarations, declaration(0)),
            DeclarationInsert::Duplicate
        );
        assert_eq!(
            insert_declaration(&mut declarations, declaration(4)),
            DeclarationInsert::Inserted
        );

        while declarations.len() < MAX_HOVER_DECLARATIONS {
            let start = u32::try_from(declarations.len()).expect("index") * 8;
            assert_eq!(
                insert_declaration(&mut declarations, declaration(start)),
                DeclarationInsert::Inserted
            );
        }
        assert_eq!(declarations.len(), MAX_HOVER_DECLARATIONS);
        let start = u32::try_from(MAX_HOVER_DECLARATIONS).expect("cap") * 8;
        assert_eq!(
            insert_declaration(&mut declarations, declaration(start)),
            DeclarationInsert::Truncated
        );
        assert_eq!(declarations.len(), MAX_HOVER_DECLARATIONS);
    }

    #[test]
    fn oversized_sets_are_rejected_not_grown() {
        let mut declarations = (0..=MAX_HOVER_DECLARATIONS)
            .map(|index| declaration(u32::try_from(index).expect("index") * 8))
            .collect::<Vec<_>>();
        assert_eq!(declarations.len(), MAX_HOVER_DECLARATIONS + 1);
        assert_eq!(
            insert_declaration(&mut declarations, declaration(999)),
            DeclarationInsert::Truncated
        );
        assert_eq!(declarations.len(), MAX_HOVER_DECLARATIONS + 1);
    }
}
