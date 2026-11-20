//! Documentation and `JSDoc` presentation.
//!
//! Both native documentation calls are independent. A genuinely unavailable
//! method is recorded and the other evidence is kept; a hard transport or
//! engine failure is propagated by the session. Two unavailable methods are an
//! explicit `Unsupported` answer, never an empty "no documentation" success.

use wxml_types::CapabilityStatus;

use crate::native::session::DocumentationQuery;

/// One `JSDoc` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsDocTagInfo {
    /// Tag name without `@`.
    pub name: String,
    /// Tag text, when present.
    pub text: Option<String>,
}

/// Documentation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentationResult {
    /// Documentation comment, preserving newlines.
    pub comment: Option<String>,
    /// `JSDoc` tags in engine order.
    pub tags: Vec<JsDocTagInfo>,
    /// Completeness of the answer.
    pub status: CapabilityStatus,
}

/// Maps raw documentation evidence.
#[must_use]
pub fn map_documentation(raw: &DocumentationQuery) -> DocumentationResult {
    let tags = raw
        .tags
        .iter()
        .map(|tag| JsDocTagInfo {
            name: tag.name.clone(),
            text: tag.text.clone(),
        })
        .collect();

    let status = match (raw.comment_failed, raw.tags_failed) {
        (true, true) => CapabilityStatus::Unsupported {
            reason: "documentation methods are unavailable".to_owned(),
        },
        (true, false) | (false, true) => CapabilityStatus::Partial {
            reason: "one documentation method failed".to_owned(),
        },
        (false, false) => CapabilityStatus::Supported,
    };

    DocumentationResult {
        comment: raw.comment.clone(),
        tags,
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::wire::RawJsDocTag;

    fn raw(comment: Option<&str>, failed: bool, tag_failed: bool) -> DocumentationQuery {
        DocumentationQuery {
            comment: comment.map(str::to_owned),
            tags: vec![RawJsDocTag {
                name: "deprecated".to_owned(),
                text: Some("use Person".to_owned()),
            }],
            comment_failed: failed,
            tags_failed: tag_failed,
        }
    }

    #[test]
    fn complete_documentation_is_supported() {
        let result = map_documentation(&raw(Some("A user."), false, false));
        assert_eq!(result.comment.as_deref(), Some("A user."));
        assert_eq!(result.tags.len(), 1);
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn partial_failure_keeps_the_other_evidence() {
        let result = map_documentation(&raw(None, true, false));
        assert!(result.comment.is_none());
        assert_eq!(result.tags.len(), 1);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));

        let result = map_documentation(&raw(Some("doc"), false, true));
        assert_eq!(result.comment.as_deref(), Some("doc"));
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn both_failures_are_unsupported_not_empty_success() {
        let result = map_documentation(&raw(None, true, true));
        assert!(matches!(
            result.status,
            CapabilityStatus::Unsupported { .. }
        ));
    }
}
