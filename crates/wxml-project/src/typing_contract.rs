//! Event contract checks between a declared component event and a handler.
//!
//! The check is static: it compares the handler's annotated parameter text with
//! the declared event detail using the same self-contained type subset as
//! [`crate::typing`]. Anything that cannot be proven is reported as skipped
//! instead of producing a diagnostic.
//!
//! The handler annotation is normalised before comparison. All of the following
//! spellings describe the same detail type:
//!
//! ```text
//! WechatMiniprogram.CustomEvent<{ index: number }>
//! CustomEvent<{ index: number }>
//! { detail: { index: number } }
//! { index: number }
//! ```
//!
//! `void`, `undefined` and `never` mean "no detail". Comparison is structural
//! equality over the parsed subset: whitespace and member separators do not
//! matter, union order does not matter, object members are compared by name
//! (the `?` marker is significant) and literals must match exactly. `any` and
//! `unknown` are compatible with every type. Only two proven, structurally
//! different types produce [`HandlerTypeCheck::Incompatible`]; the caller turns
//! that into an `Error` diagnostic.
//!
//! This module is pure: it reads no files, allocates only for the reported
//! signatures, and never panics on arbitrary input.

use std::collections::BTreeMap;

use crate::typing::{ObjectTypeMember, PrimitiveType, TypeText, TypingEvent};

/// Namespace that owns the Mini Program `CustomEvent` type.
const WECHAT_NAMESPACE: &str = "WechatMiniprogram.";

/// Local name of the standard event type.
const CUSTOM_EVENT: &str = "CustomEvent";

/// Member of an event object that carries the event detail.
const DETAIL_MEMBER: &str = "detail";

/// Detail type names that mean "the event carries no detail".
const NO_DETAIL: [&str; 3] = ["void", "undefined", "never"];

/// Why a handler annotation could not be compared with the declared detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipReason {
    /// The handler has no parameter type annotation, so nothing can be compared.
    ParameterUnannotated,
    /// The declared event detail is outside the supported type subset.
    DetailUnproven,
    /// The handler's parameter annotation is outside the supported type subset.
    AnnotationUnproven,
}

impl SkipReason {
    /// Returns the stable lower-case identifier of the reason.
    ///
    /// Callers use this as a machine-readable key when reporting a `Partial`
    /// capability state; the human-readable translation belongs to the layer
    /// that publishes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParameterUnannotated => "parameter-unannotated",
            Self::DetailUnproven => "detail-unproven",
            Self::AnnotationUnproven => "annotation-unproven",
        }
    }
}

/// Result of comparing a handler annotation with a declared event detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HandlerTypeCheck {
    /// The comparison could not be made; publish no diagnostic and degrade the
    /// capability state.
    Skipped(SkipReason),
    /// The annotation and the declared detail describe the same type.
    Compatible,
    /// Both sides are proven and structurally different.
    ///
    /// `expected` is the declared detail text and `found` is the handler
    /// parameter's detail text, both normalised (wrapper stripped, trimmed).
    Incompatible {
        /// Declared detail text, or `void` when the event has no detail.
        expected: String,
        /// Handler parameter detail text, or `void` when it declares no detail.
        found: String,
    },
}

/// Compares a handler parameter annotation with a declared event detail.
///
/// `handler_param` is the annotation text of the handler's parameter, or `None`
/// when the parameter carries no annotation. `detail_text` is
/// [`crate::typing::TypingEvent::detail_text`]; `None` means the event has no
/// detail.
///
/// Returns [`HandlerTypeCheck::Skipped`] instead of guessing whenever either
/// side is outside the supported subset. Only two proven and structurally
/// different types yield [`HandlerTypeCheck::Incompatible`].
#[must_use]
pub fn check_handler(handler_param: Option<&str>, detail_text: Option<&str>) -> HandlerTypeCheck {
    let Some(handler_param) = handler_param else {
        return HandlerTypeCheck::Skipped(SkipReason::ParameterUnannotated);
    };
    let annotated = normalize_annotation(handler_param);
    let declared = detail_text.map_or(Normalized::NoDetail, classify);
    match (annotated, declared) {
        (Normalized::Unproven, _) => HandlerTypeCheck::Skipped(SkipReason::AnnotationUnproven),
        (_, Normalized::Unproven) => HandlerTypeCheck::Skipped(SkipReason::DetailUnproven),
        (Normalized::NoDetail, Normalized::NoDetail) => HandlerTypeCheck::Compatible,
        (Normalized::Detail { text: found, ty }, Normalized::NoDetail) => {
            if is_permissive(&ty) {
                HandlerTypeCheck::Compatible
            } else {
                HandlerTypeCheck::Incompatible {
                    expected: "void".to_owned(),
                    found,
                }
            }
        }
        (Normalized::NoDetail, Normalized::Detail { text: expected, ty }) => {
            if is_permissive(&ty) {
                HandlerTypeCheck::Compatible
            } else {
                HandlerTypeCheck::Incompatible {
                    expected,
                    found: "void".to_owned(),
                }
            }
        }
        (
            Normalized::Detail {
                text: found,
                ty: found_ty,
            },
            Normalized::Detail {
                text: expected,
                ty: expected_ty,
            },
        ) => {
            if compatible(&found_ty, &expected_ty) {
                HandlerTypeCheck::Compatible
            } else {
                HandlerTypeCheck::Incompatible { expected, found }
            }
        }
    }
}

/// Renders the hover signature for one event detail.
///
/// `None` renders `(e: WechatMiniprogram.CustomEvent<void>) => void`. A detail
/// outside the supported subset keeps its declared text; the caller is
/// responsible for marking the signature as unproven.
#[must_use]
pub fn expected_signature(detail_text: Option<&str>) -> String {
    let detail = detail_text
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("void");
    format!("(e: WechatMiniprogram.CustomEvent<{detail}>) => void")
}

/// Declared event contracts of one component, looked up by event name.
///
/// The table borrows the component's declared events, so it stays cheap to
/// build while resolving an event binding. Completion treats a [`TypingEvent`]
/// as declared only when the owning component is complete; that decision stays
/// with the caller.
#[derive(Clone, Copy, Debug)]
pub struct EventContractTable<'a> {
    /// Declared events ordered by name.
    events: &'a [TypingEvent],
}

impl<'a> EventContractTable<'a> {
    /// Creates a table over one component's declared events.
    #[must_use]
    pub const fn new(events: &'a [TypingEvent]) -> Self {
        Self { events }
    }

    /// Returns the contract of one declared event.
    #[must_use]
    pub fn contract(&self, name: &str) -> Option<EventContract<'a>> {
        self.events
            .iter()
            .find(|event| event.name == name)
            .map(|event| EventContract {
                detail_text: event.detail_text.as_deref(),
                proven: event.proven,
            })
    }
}

/// One declared event's detail contract.
#[derive(Clone, Copy, Debug)]
pub struct EventContract<'a> {
    /// Detail type text as declared, or `None` when the event has no detail.
    pub detail_text: Option<&'a str>,
    /// Whether the declared detail is fully understood.
    pub proven: bool,
}

impl EventContract<'_> {
    /// Compares a handler annotation with this contract.
    ///
    /// An unproven declaration skips the comparison even when the handler is
    /// annotated, because the declared side cannot be trusted.
    #[must_use]
    pub fn check(&self, handler_param: Option<&str>) -> HandlerTypeCheck {
        if !self.proven {
            return HandlerTypeCheck::Skipped(SkipReason::DetailUnproven);
        }
        check_handler(handler_param, self.detail_text)
    }

    /// Renders the hover signature for this event.
    #[must_use]
    pub fn expected_signature(&self) -> String {
        expected_signature(self.detail_text)
    }
}

/// Normalised shape of one side of the comparison.
enum Normalized {
    /// The type means "no detail".
    NoDetail,
    /// A proven or permissive detail type together with its display text.
    Detail {
        /// Text used in diagnostics, with any wrapper already stripped.
        text: String,
        /// Parsed type expression.
        ty: TypeText,
    },
    /// A type expression outside the supported subset.
    Unproven,
}

/// Strips the event wrappers from a handler parameter annotation.
fn normalize_annotation(text: &str) -> Normalized {
    if let Some(inner) = custom_event_argument(text) {
        return classify(inner);
    }
    if let Some(inner) = detail_object_argument(text) {
        return classify(inner);
    }
    classify(text)
}

/// Classifies one detail type expression.
fn classify(text: &str) -> Normalized {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Normalized::Unproven;
    }
    if NO_DETAIL.contains(&trimmed) {
        return Normalized::NoDetail;
    }
    let ty = TypeText::parse(trimmed);
    if !ty.is_proven() {
        return Normalized::Unproven;
    }
    Normalized::Detail {
        text: trimmed.to_owned(),
        ty,
    }
}

/// Returns the generic argument of a whole `CustomEvent<...>` annotation.
///
/// Accepts both the qualified and the local spelling and requires the generic
/// list to span the whole expression, so a union or intersection that merely
/// starts with `CustomEvent<...>` stays unproven instead of being cut.
fn custom_event_argument(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let local = trimmed.strip_prefix(WECHAT_NAMESPACE).unwrap_or(trimmed);
    let local = local.trim_start();
    let after_name = local.strip_prefix(CUSTOM_EVENT)?;
    if after_name.starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$') {
        return None;
    }
    let arguments = after_name.trim_start();
    if !arguments.starts_with('<') {
        return None;
    }
    let open = trimmed.len().checked_sub(arguments.len())?;
    let close = matching_angle(trimmed, open)?;
    if !trimmed.get(close + 1..)?.trim().is_empty() {
        return None;
    }
    let inner = trimmed.get(open + 1..close)?;
    Some(first_type_argument(inner).trim())
}

/// Returns the `detail` member type of a whole `{ detail: T }` annotation.
///
/// Only the outermost object is inspected, so a detail type that itself has a
/// `detail` member survives the `CustomEvent<{ detail: T }>` spelling unchanged.
fn detail_object_argument(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let close = matching_brace(trimmed, 0)?;
    if !trimmed.get(close + 1..)?.trim().is_empty() {
        return None;
    }
    let inner = trimmed.get(1..close)?;
    let mut found = None;
    for member in split_members(inner) {
        let member = member.trim();
        if member.is_empty() {
            continue;
        }
        let colon = top_level_colon(member)?;
        let name = member.get(..colon)?.trim().trim_end_matches('?').trim();
        if unquote(name) != DETAIL_MEMBER {
            continue;
        }
        found = Some(member.get(colon + 1..)?.trim());
    }
    found
}

/// Returns whether the type accepts every other type.
const fn is_permissive(ty: &TypeText) -> bool {
    matches!(
        ty,
        TypeText::Primitive(PrimitiveType::Any | PrimitiveType::Unknown)
    )
}

/// Compares two proven type expressions by normalised structure.
fn compatible(left: &TypeText, right: &TypeText) -> bool {
    if is_permissive(left) || is_permissive(right) || left == right {
        return true;
    }
    match (left, right) {
        (TypeText::Array(left), TypeText::Array(right))
        | (TypeText::Record(left), TypeText::Record(right)) => compatible(left, right),
        (TypeText::Object(left), TypeText::Object(right)) => object_eq(left, right),
        (TypeText::Union(_), _) | (_, TypeText::Union(_)) => {
            let mut left_parts = Vec::new();
            flatten_union(left, &mut left_parts);
            let mut right_parts = Vec::new();
            flatten_union(right, &mut right_parts);
            union_eq(&left_parts, &right_parts)
        }
        _ => false,
    }
}

/// Collects the alternatives of a possibly nested union.
fn flatten_union<'a>(ty: &'a TypeText, out: &mut Vec<&'a TypeText>) {
    if let TypeText::Union(parts) = ty {
        for part in parts {
            flatten_union(part, out);
        }
    } else {
        out.push(ty);
    }
}

/// Compares two unions as unordered multisets.
fn union_eq(left: &[&TypeText], right: &[&TypeText]) -> bool {
    let left = deduplicate(left);
    let right = deduplicate(right);
    if left.len() != right.len() {
        return false;
    }
    let mut used = vec![false; right.len()];
    left.iter().all(|item| {
        let Some(index) = right
            .iter()
            .enumerate()
            .position(|(index, other)| !used[index] && compatible(item, other))
        else {
            return false;
        };
        used[index] = true;
        true
    })
}

/// Drops alternatives that are compatible with an already kept one.
fn deduplicate<'a>(parts: &[&'a TypeText]) -> Vec<&'a TypeText> {
    let mut unique: Vec<&'a TypeText> = Vec::new();
    for &part in parts {
        if !unique.iter().any(|kept| compatible(kept, part)) {
            unique.push(part);
        }
    }
    unique
}

/// Compares two object literal types by member name set.
fn object_eq(left: &[ObjectTypeMember], right: &[ObjectTypeMember]) -> bool {
    let left = member_map(left);
    let right = member_map(right);
    if left.len() != right.len() {
        return false;
    }
    left.iter().all(|(name, member)| {
        right.get(name).is_some_and(|other| {
            other.optional == member.optional && compatible(&member.ty, &other.ty)
        })
    })
}

/// Indexes object members by name, keeping the first duplicate.
fn member_map(members: &[ObjectTypeMember]) -> BTreeMap<&str, &ObjectTypeMember> {
    let mut map = BTreeMap::new();
    for member in members {
        map.entry(member.name.as_str()).or_insert(member);
    }
    map
}

/// Finds the matching `>` of a generic argument list.
fn matching_angle(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'<' => depth += 1,
            b'>' => {
                let arrow = index
                    .checked_sub(1)
                    .and_then(|previous| bytes.get(previous))
                    == Some(&b'=');
                if !arrow {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index);
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Finds the matching `}` of an object type.
fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Returns the first top-level generic argument.
fn first_type_argument(inner: &str) -> &str {
    let bytes = inner.as_bytes();
    let mut depth = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => return inner.get(..index).unwrap_or(inner),
            _ => {}
        }
        index += 1;
    }
    inner
}

/// Splits one object body on top-level `;` and `,`.
fn split_members(inner: &str) -> Vec<&str> {
    let bytes = inner.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
            b';' | b',' if depth == 0 => {
                parts.push(inner.get(start..index).unwrap_or(""));
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    parts.push(inner.get(start..).unwrap_or(""));
    parts
}

/// Returns the offset of the first top-level `:` in one member.
fn top_level_colon(member: &str) -> Option<usize> {
    let bytes = member.as_bytes();
    let mut depth = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return Some(index),
            _ => {}
        }
        index += 1;
    }
    None
}

/// Removes one pair of surrounding quotes from a member name.
fn unquote(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return text;
    }
    let Some(&quote) = bytes.first() else {
        return text;
    };
    if (quote == b'\'' || quote == b'"') && bytes.last() == Some(&quote) {
        return text.get(1..bytes.len() - 1).unwrap_or(text);
    }
    text
}

/// Returns the byte after the string literal starting at `index`.
fn string_end(bytes: &[u8], index: usize) -> usize {
    let Some(&quote) = bytes.get(index) else {
        return index;
    };
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = cursor.saturating_add(2),
            byte if byte == quote => return cursor + 1,
            _ => cursor += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one declared event.
    fn event(name: &str, detail: Option<&str>, proven: bool) -> TypingEvent {
        TypingEvent {
            name: name.to_owned(),
            detail_text: detail.map(str::to_owned),
            detail: detail.map(TypeText::parse),
            proven,
            documentation: None,
        }
    }

    #[test]
    fn accepts_compatible_annotations() {
        let cases = [
            (
                "WechatMiniprogram.CustomEvent<{ index: number }>",
                Some("{ index: number }"),
            ),
            ("CustomEvent<{index:number;}>", Some("{ index: number }")),
            (
                "CustomEvent<{ a: string; b?: number }>",
                Some("{ b?: number; a: string }"),
            ),
            ("{ detail: { index: number } }", Some("{ index: number }")),
            ("{ index: number }", Some("{ index: number }")),
            ("CustomEvent<'a' | 'b'>", Some("'b' | 'a'")),
            ("CustomEvent<string[]>", Some("string[]")),
            (
                "CustomEvent<Record<string, number>>",
                Some("Record<string, number>"),
            ),
            (
                "CustomEvent<{ items: { id: number }[] }>",
                Some("{ items: { id: number }[] }"),
            ),
            ("CustomEvent<void>", None),
            ("CustomEvent<undefined>", None),
            ("CustomEvent<never>", None),
            ("void", None),
            ("CustomEvent<any>", Some("{ index: number }")),
            ("CustomEvent<unknown>", Some("'a'")),
            (
                "CustomEvent<{ index: any }>",
                Some("{ index: { deep: string } }"),
            ),
            ("CustomEvent<{ name: string }>", Some("any")),
        ];
        for (param, detail) in cases {
            assert_eq!(
                check_handler(Some(param), detail),
                HandlerTypeCheck::Compatible,
                "param = {param:?}, detail = {detail:?}"
            );
        }
    }

    #[test]
    fn rejects_structurally_different_annotations() {
        let cases = [
            (
                "WechatMiniprogram.CustomEvent<{ name: string }>",
                "{ index: number }",
                "{ index: number }",
                "{ name: string }",
            ),
            ("CustomEvent<'small'>", "'large'", "'large'", "'small'"),
            ("CustomEvent<number>", "string", "string", "number"),
            (
                "CustomEvent<{ index?: number }>",
                "{ index: number }",
                "{ index: number }",
                "{ index?: number }",
            ),
            (
                "CustomEvent<{ a: string }>",
                "{ a: string; b: number }",
                "{ a: string; b: number }",
                "{ a: string }",
            ),
            (
                "CustomEvent<{ a: number }>",
                "{ a: string }",
                "{ a: string }",
                "{ a: number }",
            ),
            (
                "CustomEvent<void>",
                "{ index: number }",
                "{ index: number }",
                "void",
            ),
            ("void", "{ index: number }", "{ index: number }", "void"),
            ("CustomEvent<string[]>", "number[]", "number[]", "string[]"),
            (
                "CustomEvent<Record<string, string>>",
                "Record<string, number>",
                "Record<string, number>",
                "Record<string, string>",
            ),
            ("CustomEvent<1>", "2", "2", "1"),
        ];
        for (param, detail, expected, found) in cases {
            assert_eq!(
                check_handler(Some(param), Some(detail)),
                HandlerTypeCheck::Incompatible {
                    expected: expected.to_owned(),
                    found: found.to_owned(),
                },
                "param = {param:?}, detail = {detail:?}"
            );
        }
    }

    #[test]
    fn rejects_a_detail_mismatch_against_no_detail() {
        assert_eq!(
            check_handler(Some("CustomEvent<{ index: number }>"), Some("undefined")),
            HandlerTypeCheck::Incompatible {
                expected: "void".to_owned(),
                found: "{ index: number }".to_owned(),
            }
        );
    }

    #[test]
    fn skips_unannotated_parameters() {
        assert_eq!(
            check_handler(None, Some("{ index: number }")),
            HandlerTypeCheck::Skipped(SkipReason::ParameterUnannotated)
        );
        assert_eq!(
            check_handler(None, None),
            HandlerTypeCheck::Skipped(SkipReason::ParameterUnannotated)
        );
        assert_eq!(
            SkipReason::ParameterUnannotated.as_str(),
            "parameter-unannotated"
        );
    }

    #[test]
    fn skips_unproven_sides_without_reporting_a_mismatch() {
        for param in ["", "   ", "SomeLib.Options", "CustomEvent<SomeLib.Options>"] {
            assert_eq!(
                check_handler(Some(param), Some("{ index: number }")),
                HandlerTypeCheck::Skipped(SkipReason::AnnotationUnproven),
                "param = {param:?}"
            );
        }
        for detail in ["SomeLib.Options", "ReadonlyArray<string>", ""] {
            assert_eq!(
                check_handler(Some("CustomEvent<{ index: number }>"), Some(detail)),
                HandlerTypeCheck::Skipped(SkipReason::DetailUnproven),
                "detail = {detail:?}"
            );
        }
        assert_eq!(SkipReason::DetailUnproven.as_str(), "detail-unproven");
        assert_eq!(
            SkipReason::AnnotationUnproven.as_str(),
            "annotation-unproven"
        );
    }

    #[test]
    fn renders_the_expected_signature() {
        assert_eq!(
            expected_signature(None),
            "(e: WechatMiniprogram.CustomEvent<void>) => void"
        );
        assert_eq!(
            expected_signature(Some("{ index: number }")),
            "(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void"
        );
        assert_eq!(
            expected_signature(Some("  ")),
            "(e: WechatMiniprogram.CustomEvent<void>) => void"
        );
        assert_eq!(
            expected_signature(Some("CustomEvent<string>")),
            "(e: WechatMiniprogram.CustomEvent<CustomEvent<string>>) => void"
        );
    }

    #[test]
    fn checks_event_contracts_from_a_component() {
        let events = vec![
            event("change", Some("{ index: number }"), true),
            event("tap", None, true),
            event("external", None, false),
        ];
        let table = EventContractTable::new(&events);

        let change = table.contract("change").expect("declared event");
        assert_eq!(change.detail_text, Some("{ index: number }"));
        assert!(change.proven);
        assert_eq!(
            change.check(Some("WechatMiniprogram.CustomEvent<{ index: number }>")),
            HandlerTypeCheck::Compatible
        );
        assert_eq!(
            change.check(Some("WechatMiniprogram.CustomEvent<{ name: string }>")),
            HandlerTypeCheck::Incompatible {
                expected: "{ index: number }".to_owned(),
                found: "{ name: string }".to_owned(),
            }
        );
        assert_eq!(
            change.check(None),
            HandlerTypeCheck::Skipped(SkipReason::ParameterUnannotated)
        );
        assert_eq!(
            change.expected_signature(),
            "(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void"
        );

        let tap = table.contract("tap").expect("no-detail event");
        assert_eq!(tap.detail_text, None);
        assert_eq!(
            tap.check(Some("CustomEvent<void>")),
            HandlerTypeCheck::Compatible
        );
        assert_eq!(
            tap.check(Some("CustomEvent<string>")),
            HandlerTypeCheck::Incompatible {
                expected: "void".to_owned(),
                found: "string".to_owned(),
            }
        );
        assert_eq!(
            tap.expected_signature(),
            "(e: WechatMiniprogram.CustomEvent<void>) => void"
        );

        let external = table.contract("external").expect("unproven event");
        assert!(!external.proven);
        assert_eq!(
            external.check(Some("CustomEvent<{ id: number }>")),
            HandlerTypeCheck::Skipped(SkipReason::DetailUnproven)
        );

        assert!(table.contract("missing").is_none());
    }

    #[test]
    fn arbitrary_annotations_do_not_panic() {
        let inputs = [
            "",
            "{",
            "CustomEvent<",
            "CustomEvent<{",
            "{ detail: }",
            "{ detail: { a: 1 }",
            "WechatMiniprogram.",
            "WechatMiniprogram.CustomEvent",
            "CustomEvent<>",
            "CustomEvent<{ a: 1 }, { b: 2 }>",
            "CustomEvent<() => void>",
            "CustomEvent<'a>b'>",
            "CustomEvent<{ detail: 'x' }>",
            "{ 'detail': string }",
            "{ detail?: string }",
            "{ detail: string; other: number }",
            "CustomEvent<{ a: Record<string, 'x' | 'y'> }>",
        ];
        for input in inputs {
            let result = check_handler(Some(input), Some("{ index: number }"));
            assert!(
                matches!(
                    result,
                    HandlerTypeCheck::Skipped(_)
                        | HandlerTypeCheck::Compatible
                        | HandlerTypeCheck::Incompatible { .. }
                ),
                "input = {input:?}"
            );
            let signature = expected_signature(Some(input));
            assert!(signature.starts_with("(e: WechatMiniprogram.CustomEvent<"));
        }
        assert_eq!(
            check_handler(Some("CustomEvent<{ a: 1 }, { b: 2 }>"), None),
            HandlerTypeCheck::Incompatible {
                expected: "void".to_owned(),
                found: "{ a: 1 }".to_owned(),
            }
        );
        assert_eq!(
            check_handler(Some("CustomEvent<() => void>"), Some("{ a: 1 }")),
            HandlerTypeCheck::Skipped(SkipReason::AnnotationUnproven)
        );
        assert_eq!(
            check_handler(Some("{ detail: string; other: number }"), Some("string")),
            HandlerTypeCheck::Compatible
        );
        assert_eq!(
            check_handler(Some("{ detail?: string }"), Some("string")),
            HandlerTypeCheck::Compatible
        );
        assert_eq!(
            check_handler(Some("{ 'detail': string }"), Some("string")),
            HandlerTypeCheck::Compatible
        );
    }
}
