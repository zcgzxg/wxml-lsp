//! Parse diagnostics shared by the WXML and expression parsers.
//!
//! A parse error never aborts the whole document: callers always receive a
//! tree they can inspect. Errors carry exact source ranges so an editor can
//! underline the malformed region.

use std::fmt;

use wxml_text::TextRange;

/// Maximum number of regular diagnostics kept for one parse.
///
/// When a parser produces more, the last entry becomes
/// [`ParseErrorKind::TooManyErrors`] and further diagnostics are dropped. This
/// bounds memory for hostile input; the sentinel makes truncation visible
/// instead of silently dropping errors.
pub const MAX_ERRORS: usize = 128;

/// Bounded diagnostic collector shared by the markup and expression parsers.
pub(crate) struct ErrorSink {
    errors: Vec<ParseError>,
    truncated: bool,
}

impl ErrorSink {
    pub(crate) const fn new() -> Self {
        Self {
            errors: Vec::new(),
            truncated: false,
        }
    }

    pub(crate) fn push(&mut self, kind: ParseErrorKind, range: TextRange, message: &'static str) {
        if self.truncated {
            return;
        }
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(ParseError::new(kind, range, message));
            return;
        }
        self.truncate(range);
    }

    /// Merges diagnostics from another parser, keeping the cap.
    pub(crate) fn extend(&mut self, errors: Vec<ParseError>) {
        for error in errors {
            self.push_error(error);
        }
    }

    fn push_error(&mut self, error: ParseError) {
        if self.truncated {
            return;
        }
        if error.kind == ParseErrorKind::TooManyErrors {
            self.truncate(error.range);
            return;
        }
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(error);
        } else {
            self.truncate(error.range);
        }
    }

    fn truncate(&mut self, range: TextRange) {
        if self.truncated {
            return;
        }
        self.errors.push(ParseError::new(
            ParseErrorKind::TooManyErrors,
            range,
            "too many errors; diagnostics truncated",
        ));
        self.truncated = true;
    }

    pub(crate) fn into_vec(self) -> Vec<ParseError> {
        self.errors
    }
}

impl Default for ErrorSink {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a [`TextRange`] from bounds that are expected to be ordered.
///
/// `TextRange::new` only rejects `start > end`. Equal bounds are always valid,
/// so retrying with `end` as both bounds terminates immediately. This keeps the
/// fallback total without `unwrap`/`expect` in production code.
pub(crate) fn span(start: u32, end: u32) -> TextRange {
    let Ok(range) = TextRange::new(start, end) else {
        return span(end, end);
    };
    range
}

/// A single parse problem with its source range.
///
/// Errors are ordered by discovery. The same input always produces the same
/// errors in the same order, which makes golden tests and repeated parses
/// comparable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    /// Byte range in the original source text covered by the problem.
    pub range: TextRange,
    /// Stable machine-readable category.
    pub kind: ParseErrorKind,
    /// Short human-readable description. Deterministic for a given input and
    /// stored as a static string, so diagnostics never allocate per message.
    pub message: &'static str,
}

impl ParseError {
    /// Builds an error from a static message.
    pub(crate) const fn new(kind: ParseErrorKind, range: TextRange, message: &'static str) -> Self {
        Self {
            range,
            kind,
            message,
        }
    }

    /// Returns the machine-readable category.
    #[must_use]
    pub const fn kind(&self) -> ParseErrorKind {
        self.kind
    }

    /// Returns the covered byte range.
    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at {}..{}: {}",
            self.kind.as_str(),
            self.range.start(),
            self.range.end(),
            self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Stable categories for [`ParseError`].
///
/// The enum is `non_exhaustive`: new recovery cases may be added without
/// breaking callers that match on the existing ones.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ParseErrorKind {
    /// A token appeared where the grammar did not allow it.
    UnexpectedToken,
    /// An element or tag was still open at the end of the input.
    UnclosedTag,
    /// A closing tag did not match the element it appeared in.
    MismatchedTag,
    /// A closing tag appeared without a matching opening tag.
    UnexpectedClosingTag,
    /// A `{{` was never closed by `}}`.
    UnclosedMustache,
    /// A mustache contained no expression.
    EmptyMustache,
    /// A comment was not terminated before the end of the input.
    UnclosedComment,
    /// A quoted attribute value was not terminated.
    UnclosedAttributeValue,
    /// An attribute ended after `=` without a value.
    MissingAttributeValue,
    /// The expression grammar rejected the input.
    InvalidExpression,
    /// Expression nesting exceeded the supported depth.
    ExpressionTooDeep,
    /// A single expression contained too many tokens.
    ExpressionTooManyTokens,
    /// Element nesting exceeded the supported depth.
    NestingTooDeep,
    /// An input byte could not be interpreted in the current context.
    InvalidCharacter,
    /// The input is larger than the supported 4 GiB byte range.
    InputTooLarge,
    /// More diagnostics were produced than [`MAX_ERRORS`]; later ones were
    /// dropped. This is always the last diagnostic when present.
    TooManyErrors,
    /// The node budget was exhausted and the tree was truncated.
    TooManyNodes,
    /// The markup token budget was exhausted and the input was truncated.
    TooManyTokens,
}

impl ParseErrorKind {
    /// Returns the stable snake-case name of the category.
    ///
    /// The name is suitable for logs and test assertions; it is not a
    /// user-facing localised string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnexpectedToken => "unexpected_token",
            Self::UnclosedTag => "unclosed_tag",
            Self::MismatchedTag => "mismatched_tag",
            Self::UnexpectedClosingTag => "unexpected_closing_tag",
            Self::UnclosedMustache => "unclosed_mustache",
            Self::EmptyMustache => "empty_mustache",
            Self::UnclosedComment => "unclosed_comment",
            Self::UnclosedAttributeValue => "unclosed_attribute_value",
            Self::MissingAttributeValue => "missing_attribute_value",
            Self::InvalidExpression => "invalid_expression",
            Self::ExpressionTooDeep => "expression_too_deep",
            Self::ExpressionTooManyTokens => "expression_too_many_tokens",
            Self::NestingTooDeep => "nesting_too_deep",
            Self::InvalidCharacter => "invalid_character",
            Self::InputTooLarge => "input_too_large",
            Self::TooManyErrors => "too_many_errors",
            Self::TooManyNodes => "too_many_nodes",
            Self::TooManyTokens => "too_many_tokens",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_range_and_kind() {
        let range = TextRange::new(2, 5).expect("valid range");
        let error = ParseError::new(ParseErrorKind::UnclosedTag, range, "tag is not closed");
        assert_eq!(error.range(), range);
        assert_eq!(error.kind(), ParseErrorKind::UnclosedTag);
        assert_eq!(error.to_string(), "unclosed_tag at 2..5: tag is not closed");
    }

    #[test]
    fn error_kind_names_are_stable() {
        assert_eq!(ParseErrorKind::UnexpectedToken.as_str(), "unexpected_token");
        assert_eq!(ParseErrorKind::InputTooLarge.as_str(), "input_too_large");
        assert_eq!(
            ParseErrorKind::ExpressionTooManyTokens.as_str(),
            "expression_too_many_tokens"
        );
    }

    #[test]
    fn span_keeps_ordered_bounds() {
        let range = span(3, 9);
        assert_eq!(range.start(), 3);
        assert_eq!(range.end(), 9);
    }

    #[test]
    fn span_collapses_inverted_bounds_instead_of_panicking() {
        let range = span(9, 3);
        assert_eq!(range.start(), 3);
        assert_eq!(range.end(), 3);
    }

    #[test]
    fn error_sink_caps_diagnostics_and_keeps_a_sentinel() {
        let range = TextRange::new(0, 1).expect("valid range");
        let mut sink = ErrorSink::new();
        for _ in 0..MAX_ERRORS * 2 {
            sink.push(ParseErrorKind::UnexpectedToken, range, "bad token");
        }
        let errors = sink.into_vec();
        assert_eq!(errors.len(), MAX_ERRORS + 1);
        assert!(
            errors[..MAX_ERRORS]
                .iter()
                .all(|error| error.kind == ParseErrorKind::UnexpectedToken)
        );
        let last = errors.last().expect("sentinel");
        assert_eq!(last.kind, ParseErrorKind::TooManyErrors);
        assert_eq!(last.message, "too many errors; diagnostics truncated");
    }

    #[test]
    fn error_sink_keeps_original_diagnostics_before_the_cap() {
        let range = TextRange::new(3, 4).expect("valid range");
        let mut sink = ErrorSink::new();
        sink.push(ParseErrorKind::UnclosedTag, range, "unclosed");
        sink.push(ParseErrorKind::MismatchedTag, range, "mismatch");
        let errors = sink.into_vec();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].kind, ParseErrorKind::UnclosedTag);
        assert_eq!(errors[1].kind, ParseErrorKind::MismatchedTag);
    }

    #[test]
    fn error_sink_merges_truncated_expression_diagnostics() {
        let range = TextRange::new(0, 1).expect("valid range");
        let mut inner = ErrorSink::new();
        for _ in 0..MAX_ERRORS + 16 {
            inner.push(ParseErrorKind::InvalidCharacter, range, "invalid");
        }
        let mut outer = ErrorSink::new();
        outer.extend(inner.into_vec());
        let errors = outer.into_vec();
        assert_eq!(errors.len(), MAX_ERRORS + 1);
        assert_eq!(
            errors.last().map(|error| error.kind),
            Some(ParseErrorKind::TooManyErrors)
        );
    }
}
