//! Classification shared by static and Node-backed completion.

use std::cmp::Ordering;

use wxml_text::TextRange;
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
///
/// Event values are classified by attribute shape only; the value itself is
/// classified by [`event_value_route`] so a malformed value can be reported
/// as unsupported instead of silently falling back to static suggestions.
pub(crate) fn route(context: &Context) -> Route {
    match &context.site {
        Site::Tag { .. } | Site::AttributeName { .. } => Route::Static,
        Site::AttributeValue { attribute, .. } => {
            if super::catalog::is_event_value_attribute(attribute) {
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

/// Precise routing result for one site on an event-binding attribute.
///
/// This is the S0 contract consumed by the S2 native event-handler fast
/// branch. It only classifies template text; it never prepares a view, starts
/// a native session or consults a backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EventValueRoute {
    /// The site is not an event-binding value.
    NotEvent,
    /// The cursor completes a handler-name prefix.
    Handler(EventValuePrefix),
    /// The attribute binds an event, but this value cannot be served.
    ///
    /// Callers must return an empty `Unsupported` result for this case, not a
    /// static value completion.
    Unsupported(EventValueRejection),
}

/// Replacement range and typed prefix for an event-handler completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventValuePrefix {
    /// `trimmed token start..cursor`.
    ///
    /// Quoting, leading spaces and any typed tail after the cursor survive;
    /// this is narrower than [`Context::replace`].
    pub(crate) replace: TextRange,
    /// Handler-name text between the token start and the cursor.
    pub(crate) prefix: String,
}

/// Why an event-binding value cannot serve handler completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventValueRejection {
    /// The cursor is outside the attribute value.
    OutsideValue,
    /// [`crate::styles::value_bounds`] disagrees with the detected quote state.
    QuoteMismatch,
    /// The value contains a mustache or crosses a line.
    DynamicValue,
    /// The cursor sits before the first non-space byte of the value.
    BeforeToken,
    /// The cursor sits at the token start; completion would prepend a name.
    AtTokenStart,
    /// The cursor sits after the trimmed token.
    AfterToken,
    /// The trimmed value is not a single identifier-shaped handler name.
    InvalidHandlerToken,
    /// The value bounds or offsets cannot be represented.
    Unrepresentable,
}

/// Classifies an event-binding attribute value for handler completion.
///
/// Returns [`EventValueRoute::NotEvent`] for non-event sites. Event sites
/// report either the exact prefix replacement or the reason the value cannot
/// be served, so callers never silently fall back to static completion.
///
/// The quote is authoritative only when [`crate::styles::value_bounds`]
/// reports one: `bindtap=|"onTap"` is an unquoted value containing quotes and
/// is rejected as a handler token, even though the cursor follows an `=`.
pub(crate) fn event_value_route(text: &str, offset: u32, context: &Context) -> EventValueRoute {
    let Site::AttributeValue {
        attribute, quoted, ..
    } = &context.site
    else {
        return EventValueRoute::NotEvent;
    };
    if !super::catalog::is_event_value_attribute(attribute) {
        return EventValueRoute::NotEvent;
    }
    match event_value_prefix(text, offset, context, *quoted) {
        Ok(prefix) => EventValueRoute::Handler(prefix),
        Err(reason) => EventValueRoute::Unsupported(reason),
    }
}

/// Computes the handler prefix and its replacement range inside one value.
fn event_value_prefix(
    text: &str,
    offset: u32,
    context: &Context,
    site_quoted: bool,
) -> Result<EventValuePrefix, EventValueRejection> {
    let Some((value, quote)) = crate::styles::value_bounds(text, offset, context) else {
        return Err(EventValueRejection::Unrepresentable);
    };
    if quote.is_some() != site_quoted {
        return Err(EventValueRejection::QuoteMismatch);
    }
    let cursor = usize::try_from(offset).map_err(|_| EventValueRejection::Unrepresentable)?;
    let value_start =
        usize::try_from(value.start()).map_err(|_| EventValueRejection::Unrepresentable)?;
    let value_end =
        usize::try_from(value.end()).map_err(|_| EventValueRejection::Unrepresentable)?;
    if cursor < value_start || cursor > value_end {
        return Err(EventValueRejection::OutsideValue);
    }
    let value_text = text
        .get(value_start..value_end)
        .ok_or(EventValueRejection::Unrepresentable)?;
    if value_text.contains("{{")
        || value_text.contains("}}")
        || value_text.contains('\n')
        || value_text.contains('\r')
    {
        return Err(EventValueRejection::DynamicValue);
    }
    let leading = value_text.len() - value_text.trim_start_matches([' ', '\t']).len();
    let trimmed = value_text.trim_matches([' ', '\t']);
    let trim_start = value_start + leading;
    let trim_end = trim_start + trimmed.len();
    if trim_start == trim_end {
        return match cursor.cmp(&trim_start) {
            Ordering::Equal => Ok(EventValuePrefix {
                replace: TextRange::new(offset, offset)
                    .map_err(|_| EventValueRejection::Unrepresentable)?,
                prefix: String::new(),
            }),
            Ordering::Less => Err(EventValueRejection::BeforeToken),
            Ordering::Greater => Err(EventValueRejection::AfterToken),
        };
    }
    let token = text
        .get(trim_start..trim_end)
        .ok_or(EventValueRejection::Unrepresentable)?;
    if !wxml_backend_tsgo::shadow::expression::is_identifier_name(token) {
        return Err(EventValueRejection::InvalidHandlerToken);
    }
    if cursor < trim_start {
        return Err(EventValueRejection::BeforeToken);
    }
    if cursor == trim_start {
        return Err(EventValueRejection::AtTokenStart);
    }
    if cursor > trim_end {
        return Err(EventValueRejection::AfterToken);
    }
    let prefix = text
        .get(trim_start..cursor)
        .ok_or(EventValueRejection::Unrepresentable)?
        .to_owned();
    Ok(EventValuePrefix {
        replace: TextRange::new(
            u32::try_from(trim_start).map_err(|_| EventValueRejection::Unrepresentable)?,
            offset,
        )
        .map_err(|_| EventValueRejection::Unrepresentable)?,
        prefix,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::completion::catalog::{self, EventAttributeClass, EventAttributeRejection};
    use crate::completion::context;

    /// Splits a `|`-marked source into text and cursor byte offset.
    ///
    /// Sources without a marker put the cursor at end of file.
    fn detect_at(source: &str) -> (String, u32) {
        source.find('|').map_or_else(
            || {
                (
                    source.to_owned(),
                    u32::try_from(source.len()).expect("offset"),
                )
            },
            |offset| {
                let mut text = String::with_capacity(source.len() - 1);
                text.push_str(&source[..offset]);
                text.push_str(&source[offset + 1..]);
                (text, u32::try_from(offset).expect("offset"))
            },
        )
    }

    fn handler_at(source: &str) -> (String, EventValuePrefix) {
        let (text, offset) = detect_at(source);
        let context = context::detect(&text, offset).expect("detected site");
        assert_eq!(
            route(&context),
            Route::Backend(CompletionKind::EventHandler),
            "{source}"
        );
        match event_value_route(&text, offset, &context) {
            EventValueRoute::Handler(prefix) => (text, prefix),
            other => panic!("expected a handler prefix, got {other:?}: {source}"),
        }
    }

    fn assert_exact_prefix(source: &str, expected: &str) {
        let (text, offset) = detect_at(source);
        let (text_after, prefix) = handler_at(source);
        assert_eq!(text_after, text, "{source}");
        assert_eq!(prefix.prefix, expected, "{source}");
        assert_eq!(prefix.replace.end(), offset, "{source}");
        let start = usize::try_from(prefix.replace.start()).expect("start");
        let end = usize::try_from(prefix.replace.end()).expect("end");
        assert_eq!(&text[start..end], expected, "{source}");
        assert_eq!(
            usize::try_from(prefix.replace.len()).expect("len"),
            expected.len(),
            "{source}"
        );
    }

    fn assert_unsupported(source: &str, reason: EventValueRejection) {
        let (text, offset) = detect_at(source);
        let context = context::detect(&text, offset).expect("detected site");
        assert_eq!(
            route(&context),
            Route::Backend(CompletionKind::EventHandler),
            "{source}"
        );
        assert_eq!(
            event_value_route(&text, offset, &context),
            EventValueRoute::Unsupported(reason),
            "{source}"
        );
    }

    #[test]
    fn every_direct_event_name_routes_to_the_handler_backend() {
        let names = [
            "tap",
            "getphonenumber",
            "chooseavatar",
            "play",
            "pause",
            "ended",
            "waiting",
            "progress",
            "fullscreenchange",
            "markertap",
            "callouttap",
            "controltap",
            "poitap",
            "anchorpointtap",
            "dragstart",
            "dragend",
        ];
        for name in names {
            let attribute = format!("bind{name}");
            assert!(catalog::is_event_value_attribute(&attribute), "{attribute}");
            assert_exact_prefix(&format!("<view {attribute}=\"on|\" />"), "on");
        }
    }

    #[test]
    fn colon_capture_and_mut_bindings_are_handlers() {
        for attribute in [
            "bind:tap",
            "catch:tap",
            "capture-bind:tap",
            "capture-catch:tap",
            "mut-bind:tap",
            "bind:my-event",
        ] {
            assert!(catalog::is_event_value_attribute(attribute), "{attribute}");
            assert_exact_prefix(&format!("<view {attribute}=\"on|\" />"), "on");
        }
    }

    #[test]
    fn bare_conflicting_and_hyphenated_attributes_are_rejected() {
        let cases = [
            ("bind", EventAttributeRejection::MissingEventName),
            ("catch", EventAttributeRejection::MissingEventName),
            ("capture-bind", EventAttributeRejection::MissingEventName),
            ("capture-catch", EventAttributeRejection::MissingEventName),
            ("mut-bind", EventAttributeRejection::MissingEventName),
            ("bind:", EventAttributeRejection::EmptyEventName),
            ("catch:", EventAttributeRejection::EmptyEventName),
            ("bind::tap", EventAttributeRejection::InvalidEventName),
            ("bind:foo:bar", EventAttributeRejection::InvalidEventName),
            ("bindex", EventAttributeRejection::ConflictingAttribute),
            (
                "bind-get-phonenumber",
                EventAttributeRejection::InvalidDirectSuffix,
            ),
            ("binding-mode", EventAttributeRejection::InvalidDirectSuffix),
        ];
        for (attribute, rejection) in cases {
            assert_eq!(
                catalog::classify_event_attribute(attribute),
                EventAttributeClass::Rejected(rejection),
                "{attribute}"
            );
            assert!(!catalog::is_event_value_attribute(attribute), "{attribute}");
            let source = format!("<view {attribute}=\"on|\" />");
            let (text, offset) = detect_at(&source);
            let context = context::detect(&text, offset).expect("attribute value");
            assert_eq!(route(&context), Route::Static, "{attribute}");
            assert_eq!(
                event_value_route(&text, offset, &context),
                EventValueRoute::NotEvent,
                "{attribute}"
            );
        }
        for attribute in ["", "bi", "class", "wx:if", "data-onTap"] {
            assert_eq!(
                catalog::classify_event_attribute(attribute),
                EventAttributeClass::Other,
                "{attribute}"
            );
        }
    }

    #[test]
    fn quoted_empty_unclosed_and_spaced_values_keep_exact_ranges() {
        assert_exact_prefix("<view bindtap=\"|\" />", "");
        assert_exact_prefix("<view bindtap=\"on|\" />", "on");
        assert_exact_prefix("<view bindtap='on|' />", "on");
        assert_exact_prefix("<view bindtap=on|>", "on");
        assert_exact_prefix("<view bindtap=|>", "");
        assert_exact_prefix("<view bindtap=|", "");
        assert_exact_prefix("<view bindtap=\"on", "on");
        assert_exact_prefix("<view bindtap=\"  on|\" />", "on");
        assert_exact_prefix("<view bindtap=\"  |\" />", "");
        assert_exact_prefix("<view bindtap=\"onTap|\" />", "onTap");
        // Frozen prefix replacement: only `on` is replaced, `Tap` survives.
        assert_exact_prefix("<view bindtap=\"on|Tap\" />", "on");
    }

    #[test]
    fn unterminated_quotes_and_empty_values_are_handlers() {
        for source in [
            "<view bindtap=\"|\" />",
            "<view bindtap=\"on",
            "<view bindtap=|>",
            "<view bindtap=|",
            "<view bindtap=\"  |\" />",
        ] {
            let (text, offset) = detect_at(source);
            let context = context::detect(&text, offset).expect("value site");
            assert_eq!(
                route(&context),
                Route::Backend(CompletionKind::EventHandler),
                "{source}"
            );
            assert!(matches!(
                event_value_route(&text, offset, &context),
                EventValueRoute::Handler(_)
            ));
        }
    }

    #[test]
    fn malformed_values_are_unsupported_not_static() {
        assert_unsupported(
            "<view bindtap=|\"onTap\"",
            EventValueRejection::InvalidHandlerToken,
        );
        assert_unsupported(
            "<view bindtap=\"|onTap\" />",
            EventValueRejection::AtTokenStart,
        );
        assert_unsupported(
            "<view bindtap=\"onTap |\" />",
            EventValueRejection::AfterToken,
        );
        assert_unsupported(
            "<view bindtap=\" | onTap\" />",
            EventValueRejection::BeforeToken,
        );
        assert_unsupported(
            "<view bindtap=\"  |onTap\" />",
            EventValueRejection::AtTokenStart,
        );
        assert_unsupported(
            "<view bindtap=\"onTap()|\" />",
            EventValueRejection::InvalidHandlerToken,
        );
        assert_unsupported(
            "<view bindtap=\"item.onTap|\" />",
            EventValueRejection::InvalidHandlerToken,
        );
        assert_unsupported(
            "<view bindtap=\"onTap extra|\" />",
            EventValueRejection::InvalidHandlerToken,
        );
        assert_unsupported(
            "<view bindtap=\"on-|\" />",
            EventValueRejection::InvalidHandlerToken,
        );
        assert_unsupported(
            "<view bindtap=\"123|\" />",
            EventValueRejection::InvalidHandlerToken,
        );
        assert_unsupported(
            "<view bindtap=\"onT{{x}}|\" />",
            EventValueRejection::DynamicValue,
        );
        assert_unsupported(
            "<view bindtap=\"pre{{onT}}|\" />",
            EventValueRejection::DynamicValue,
        );
    }

    #[test]
    fn quote_state_comes_from_value_bounds() {
        // An unterminated mustache is never an event value site, even though
        // the tolerant scanner may report the expression itself.
        let (text, offset) = detect_at("<view bindtap=\"{{|");
        if let Some(context) = context::detect(&text, offset) {
            assert!(
                matches!(context.site, Site::Mustache),
                "unclosed mustache was classified as {:?}",
                context.site
            );
            assert_eq!(
                route(&context),
                Route::Backend(CompletionKind::ExpressionMember)
            );
            assert_eq!(
                event_value_route(&text, offset, &context),
                EventValueRoute::NotEvent
            );
        }

        // A complete mustache is an expression member, not a handler value.
        let (text, offset) = detect_at("<view bindtap=\"{{ onT| }}\" />");
        let context = context::detect(&text, offset).expect("mustache");
        assert!(matches!(context.site, Site::Mustache));
        assert_eq!(
            route(&context),
            Route::Backend(CompletionKind::ExpressionMember)
        );
        assert_eq!(
            event_value_route(&text, offset, &context),
            EventValueRoute::NotEvent
        );

        // Defensive quote disagreement is rejected, not treated as unquoted.
        let (text, offset) = detect_at("<view bindtap=\"on|\" />");
        let mut context = context::detect(&text, offset).expect("value site");
        if let Site::AttributeValue { quoted, .. } = &mut context.site {
            *quoted = false;
        }
        assert_eq!(
            event_value_route(&text, offset, &context),
            EventValueRoute::Unsupported(EventValueRejection::QuoteMismatch)
        );
    }

    #[test]
    fn cursor_after_a_closed_value_is_not_an_event_site() {
        for source in ["<view bindtap=\"onTap\"|", "<view bindtap=\"onTap\" |"] {
            let (text, offset) = detect_at(source);
            if let Some(context) = context::detect(&text, offset) {
                assert!(
                    !matches!(context.site, Site::AttributeValue { .. }),
                    "{source}"
                );
                assert_eq!(
                    event_value_route(&text, offset, &context),
                    EventValueRoute::NotEvent,
                    "{source}"
                );
            }
        }
    }

    #[test]
    fn replacement_offsets_survive_cjk_text_and_identifiers() {
        // Multi-byte prose before the tag must not shift the byte range.
        let source = "<view>中文文本</view><view bindtap=\"  on|\" />";
        assert_exact_prefix(source, "on");
        assert_exact_prefix("🙂<view bindtap=\"on|\" />", "on");

        // Unicode identifier prefixes are handler names, not byte slices that
        // may split a code point.
        assert_exact_prefix("<view bindtap=\"保存|\" />", "保存");
        assert_exact_prefix("<view catch:tap=\"处理|\" />", "处理");
    }

    #[test]
    fn non_event_values_stay_static() {
        for attribute in ["wx:if", "class", "data-onTap", "binding-mode", "bindex"] {
            let source = format!("<view {attribute}=\"on|\" />");
            let (text, offset) = detect_at(&source);
            let context = context::detect(&text, offset).expect("value site");
            assert_eq!(route(&context), Route::Static, "{attribute}");
            assert_eq!(
                event_value_route(&text, offset, &context),
                EventValueRoute::NotEvent,
                "{attribute}"
            );
        }
    }

    #[test]
    fn attribute_name_completion_is_not_stolen_by_event_handler() {
        for source in [
            "<view bindt|",
            "<view bind:|",
            "<view catcht|",
            "<view capture-bind:|",
        ] {
            let (text, offset) = detect_at(source);
            let context = context::detect(&text, offset).expect("attribute name");
            assert!(
                matches!(context.site, Site::AttributeName { .. }),
                "{source}"
            );
            assert_eq!(route(&context), Route::Static, "{source}");
            assert_eq!(
                event_value_route(&text, offset, &context),
                EventValueRoute::NotEvent,
                "{source}"
            );
        }
        // The attribute-name completion predicate still accepts partial names.
        assert!(catalog::is_event_prefix("bindt"));
        assert!(catalog::is_event_prefix("bind:"));
    }
}
