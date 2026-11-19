//! Type query presentation and quality classification.
//!
//! The native API returns `null` for "no type at this position" and an opaque
//! type object otherwise. `typeToString` supplies the display text. This
//! module keeps `Missing`, `Any`, `Unknown` and `Known` distinct; it never
//! fabricates a type when the engine returned `null` text from a successful
//! call. A hard `typeToString` failure is propagated by the session instead of
//! being downgraded to a partial type.

use wxml_types::CapabilityStatus;

use crate::native::session::TypeQuery;

/// Quality of a type answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeQuality {
    /// The engine returned no type at the position.
    Missing,
    /// The type is `any`.
    Any,
    /// The type is an error/unknown or has no usable text.
    Unknown,
    /// A concrete type with usable text.
    Known,
}

/// One concrete type answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    /// Display text from `typeToString`.
    pub text: String,
    /// Opaque engine type id.
    pub handle: u64,
    /// Native type flags.
    pub flags: u64,
}

/// Type query result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeResult {
    /// Concrete type evidence, absent for `Missing`.
    pub info: Option<TypeInfo>,
    /// Type quality.
    pub quality: TypeQuality,
    /// Completeness of the answer.
    pub status: CapabilityStatus,
}

/// Maps raw type evidence into the C0-facing result.
#[must_use]
pub fn map_type_query(raw: &TypeQuery) -> TypeResult {
    let Some(info) = &raw.info else {
        return TypeResult {
            info: None,
            quality: TypeQuality::Missing,
            status: CapabilityStatus::Supported,
        };
    };

    let text = raw.text.as_deref().map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return TypeResult {
            info: None,
            quality: TypeQuality::Unknown,
            status: CapabilityStatus::Partial {
                reason: "typeToString returned no usable text".to_owned(),
            },
        };
    }

    let intrinsic = info.intrinsic_name.as_deref().unwrap_or_default();
    let quality = if intrinsic == "error" || intrinsic == "unknown" || text == "unknown" {
        TypeQuality::Unknown
    } else if intrinsic == "any" || text == "any" {
        TypeQuality::Any
    } else {
        TypeQuality::Known
    };

    TypeResult {
        info: Some(TypeInfo {
            text: text.to_owned(),
            handle: info.id,
            flags: info.flags,
        }),
        quality,
        status: CapabilityStatus::Supported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::wire::RawType;

    fn query(id: Option<u64>, intrinsic: Option<&str>, text: Option<&str>) -> TypeQuery {
        TypeQuery {
            info: id.map(|id| RawType {
                id,
                flags: 1,
                intrinsic_name: intrinsic.map(str::to_owned),
            }),
            text: text.map(str::to_owned),
        }
    }

    #[test]
    fn null_is_missing_but_supported() {
        let result = map_type_query(&query(None, None, None));
        assert_eq!(result.quality, TypeQuality::Missing);
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert!(result.info.is_none());
    }

    #[test]
    fn classifies_any_error_and_known_types() {
        let any = map_type_query(&query(Some(65), Some("any"), Some("any")));
        assert_eq!(any.quality, TypeQuality::Any);
        assert_eq!(any.info.expect("info").text, "any");

        let error = map_type_query(&query(Some(66), Some("error"), Some("error")));
        assert_eq!(error.quality, TypeQuality::Unknown);

        let known = map_type_query(&query(Some(67), None, Some("User")));
        assert_eq!(known.quality, TypeQuality::Known);
        assert_eq!(known.info.expect("info").text, "User");
    }

    #[test]
    fn literal_unknown_is_unknown_quality_not_known() {
        let result = map_type_query(&query(Some(68), None, Some("unknown")));
        assert_eq!(result.quality, TypeQuality::Unknown);
        assert_eq!(result.info.as_ref().expect("info").text, "unknown");
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn intrinsic_unknown_is_unknown_quality_even_with_other_text() {
        let result = map_type_query(&query(Some(69), Some("unknown"), Some("User | unknown")));
        assert_eq!(result.quality, TypeQuality::Unknown);
        assert_eq!(result.info.as_ref().expect("info").text, "User | unknown");
    }

    #[test]
    fn user_error_text_is_known_but_engine_error_and_empty_stay_unknown() {
        // A user-declared type named `error` has no `intrinsicName`; its text
        // alone must stay known. Error and unknown intrinsics are classified
        // separately by their engine identity.
        let user_error = map_type_query(&query(Some(70), None, Some("error")));
        assert_eq!(user_error.quality, TypeQuality::Known);
        assert_eq!(user_error.info.as_ref().expect("info").text, "error");

        let empty = map_type_query(&query(Some(71), None, Some("   ")));
        assert_eq!(empty.quality, TypeQuality::Unknown);
        assert!(empty.info.is_none());
        assert!(matches!(empty.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn missing_text_is_partial_not_fabricated() {
        let result = map_type_query(&query(Some(67), None, None));
        assert_eq!(result.quality, TypeQuality::Unknown);
        assert!(result.info.is_none());
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }
}
