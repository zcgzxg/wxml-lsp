//! Minimal numeric version comparison for Node and package releases.

use std::cmp::Ordering;
use std::fmt;

use crate::error::{DeliveryError, Result};

/// A `major.minor.patch` numeric version.
///
/// Pre-release and build suffixes are accepted after the numeric core. They are
/// ignored for minimum-runtime comparisons, which is enough for Node's
/// `vMAJOR.MINOR.PATCH` output. Package release selection uses the numeric
/// core and then the original string for a stable tie-break.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Semver {
    /// Major component.
    pub major: u64,
    /// Minor component.
    pub minor: u64,
    /// Patch component.
    pub patch: u64,
}

impl Semver {
    /// Parses `1.2.3`, optionally prefixed with `v` and followed by a suffix.
    ///
    /// # Errors
    ///
    /// Returns an error when the numeric core is missing or malformed.
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let core_end = trimmed.find(['-', '+', ' ']).unwrap_or(trimmed.len());
        let core = &trimmed[..core_end];
        let mut parts = core.split('.');
        let major = parse_part(parts.next(), input)?;
        let minor = parse_part(parts.next(), input)?;
        let patch = parse_part(parts.next(), input)?;
        if parts.next().is_some() {
            return Err(DeliveryError::InvalidArgument(format!(
                "`{input}` is not a numeric version"
            )));
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Semver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn parse_part(part: Option<&str>, input: &str) -> Result<u64> {
    let part = part.ok_or_else(|| {
        DeliveryError::InvalidArgument(format!("`{input}` is not a numeric version"))
    })?;
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DeliveryError::InvalidArgument(format!(
            "`{input}` is not a numeric version"
        )));
    }
    part.parse::<u64>().map_err(|_| {
        DeliveryError::InvalidArgument(format!("`{input}` has an oversized version component"))
    })
}

/// Orders two version strings by numeric core, then by the original text.
#[must_use]
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    match (Semver::parse(left), Semver::parse(right)) {
        (Ok(left), Ok(right)) => left
            .cmp(&right)
            .then_with(|| left.to_string().cmp(&right.to_string())),
        _ => left.cmp(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v_prefix_and_suffixes() {
        assert_eq!(
            Semver::parse("v24.22.2").unwrap(),
            Semver {
                major: 24,
                minor: 22,
                patch: 2
            }
        );
        assert_eq!(
            Semver::parse("0.1.0-rc.1").unwrap(),
            Semver {
                major: 0,
                minor: 1,
                patch: 0
            }
        );
    }

    #[test]
    fn rejects_non_numeric_versions() {
        for value in ["", "v", "1", "1.2", "1.2.3.4", "1.x.3", "1.2-beta"] {
            assert!(Semver::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn compares_numeric_cores() {
        assert_eq!(
            Semver::parse("24.0.0").unwrap(),
            Semver::parse("24.0.0").unwrap()
        );
        assert!(Semver::parse("24.1.0").unwrap() > Semver::parse("24.0.9").unwrap());
        assert_eq!(compare_versions("0.2.0", "0.10.0"), Ordering::Less);
    }
}
