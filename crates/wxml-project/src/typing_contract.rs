//! Event detail rendering for hover signatures and completion.
//!
//! Whether a handler parameter is assignable to a declared event detail is no
//! longer compared here: the TypeScript engine decides it through the event
//! contract probe, using the declaration's real type identity. This module only
//! renders the declared detail into the signature shown in hovers and
//! completion items.
//!
//! `None` or a blank detail means "no detail" and renders `CustomEvent<void>`.
//! A detail outside the supported subset keeps its declared text; the caller is
//! responsible for marking the signature as unproven.

/// Renders the hover signature for one event detail.
///
/// `None` renders `(e: WechatMiniprogram.CustomEvent<void>) => void`.
#[must_use]
pub fn expected_signature(detail_text: Option<&str>) -> String {
    let detail = detail_text
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("void");
    format!("(e: WechatMiniprogram.CustomEvent<{detail}>) => void")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
