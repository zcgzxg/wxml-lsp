//! Capability reporting shared by every backend-adjacent feature.
//!
//! A capability is never implied by the presence of a request method: a
//! backend may implement `hover` yet only support part of the template
//! language. [`CapabilityStatus`] keeps that difference visible to callers and
//! to diagnostics.
//!
//! Meanings:
//!
//! - [`CapabilityStatus::Supported`]: checked for every form covered by the
//!   samples.
//! - [`CapabilityStatus::Partial`]: works for some inputs; `reason` names the
//!   limits.
//! - [`CapabilityStatus::Unsupported`]: the language form is not supported.
//! - [`CapabilityStatus::Disabled`]: the policy turned the check off.
//! - [`CapabilityStatus::Unavailable`]: the backend or its runtime is not
//!   available (exited, timed out, not started).
//! - [`CapabilityStatus::Unknown`]: support has not been established.
//!
//! The status describes check quality and availability. A failed request is
//! reported with [`crate::BackendError`] instead; do not encode transport
//! failures as a status.

use serde::{Deserialize, Serialize};

/// How well a feature is supported by the current backend and policy.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// The feature is available for every form covered by the samples.
    Supported,
    /// The feature works for some inputs; `reason` names the limits.
    Partial {
        /// Human-readable description of the supported subset.
        reason: String,
    },
    /// The language form is deliberately not supported; `reason` explains why.
    Unsupported {
        /// Human-readable description of the missing support.
        reason: String,
    },
    /// The check is turned off by policy.
    Disabled {
        /// Human-readable description of the policy that disabled it.
        reason: String,
    },
    /// The backend or its runtime is currently unavailable.
    Unavailable {
        /// Human-readable description of the failure.
        reason: String,
    },
    /// Support has not been established, so no claim is made.
    Unknown {
        /// Optional explanation for why support is unknown.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl CapabilityStatus {
    /// Returns `true` only for [`CapabilityStatus::Supported`].
    ///
    /// `Partial`, `Disabled`, `Unavailable` and `Unknown` are all reported as
    /// not supported so callers cannot silently treat a degraded result as a
    /// complete check.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }

    /// Returns the explanation attached to a degraded status.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Supported => None,
            Self::Partial { reason }
            | Self::Unsupported { reason }
            | Self::Disabled { reason }
            | Self::Unavailable { reason } => Some(reason),
            Self::Unknown { reason } => reason.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_statuses() -> [CapabilityStatus; 6] {
        [
            CapabilityStatus::Supported,
            CapabilityStatus::Partial {
                reason: "partial".to_owned(),
            },
            CapabilityStatus::Unsupported {
                reason: "unsupported".to_owned(),
            },
            CapabilityStatus::Disabled {
                reason: "disabled".to_owned(),
            },
            CapabilityStatus::Unavailable {
                reason: "unavailable".to_owned(),
            },
            CapabilityStatus::Unknown {
                reason: Some("unknown".to_owned()),
            },
        ]
    }

    #[test]
    fn supported_features_are_fully_available() {
        let status = CapabilityStatus::Supported;
        assert!(status.is_supported());
        assert_eq!(status.reason(), None);
    }

    #[test]
    fn degraded_statuses_are_not_supported() {
        for status in all_statuses().into_iter().skip(1) {
            assert!(!status.is_supported(), "{status:?}");
            assert!(status.reason().is_some(), "{status:?}");
        }
    }

    #[test]
    fn unknown_without_a_reason_has_no_explanation() {
        let status = CapabilityStatus::Unknown { reason: None };
        assert!(!status.is_supported());
        assert_eq!(status.reason(), None);
    }

    #[test]
    fn every_variant_round_trips_through_json() {
        for status in all_statuses() {
            let json = serde_json::to_string(&status).expect("serialize");
            let back: CapabilityStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, status);
        }
    }

    #[test]
    fn uses_a_self_describing_json_shape() {
        assert_eq!(
            serde_json::to_string(&CapabilityStatus::Supported).expect("serialize"),
            r#"{"status":"supported"}"#
        );
        assert_eq!(
            serde_json::to_string(&CapabilityStatus::Partial {
                reason: "why".to_owned(),
            })
            .expect("serialize"),
            r#"{"status":"partial","reason":"why"}"#
        );
        assert_eq!(
            serde_json::to_string(&CapabilityStatus::Disabled {
                reason: "policy".to_owned(),
            })
            .expect("serialize"),
            r#"{"status":"disabled","reason":"policy"}"#
        );
        assert_eq!(
            serde_json::to_string(&CapabilityStatus::Unavailable {
                reason: "exit".to_owned(),
            })
            .expect("serialize"),
            r#"{"status":"unavailable","reason":"exit"}"#
        );
        assert_eq!(
            serde_json::to_string(&CapabilityStatus::Unknown { reason: None }).expect("serialize"),
            r#"{"status":"unknown"}"#
        );
    }
}
