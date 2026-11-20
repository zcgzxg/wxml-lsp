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

/// Returns whether a version has a pre-release suffix after the numeric core.
#[must_use]
pub fn is_prerelease(version: &str) -> bool {
    let before_build = version.split_once('+').map_or(version, |(core, _)| core);
    before_build
        .split_once('-')
        .is_some_and(|(_, suffix)| !suffix.is_empty())
}

/// Validates strict semantic versioning (`MAJOR.MINOR.PATCH[-pre][+build]`).
///
/// Unlike [`Semver::parse`], this rejects a leading `v`, whitespace, missing
/// components, leading zeroes in the numeric core or numeric pre-release
/// identifiers, and empty dot-separated identifiers.
///
/// # Errors
///
/// Returns [`DeliveryError::InvalidArgument`] for a non-semver value.
pub fn validate_strict(input: &str) -> Result<()> {
    if input.is_empty() || input.len() > 128 {
        return Err(DeliveryError::InvalidArgument(
            "version must be 1-128 bytes".to_owned(),
        ));
    }
    if input.trim() != input || input.starts_with('v') {
        return Err(DeliveryError::InvalidArgument(format!(
            "`{input}` is not a strict semantic version"
        )));
    }
    let (without_build, build) = match input.split_once('+') {
        Some((core, build)) => {
            if build.is_empty() || build.contains('+') {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{input}` has a malformed build suffix"
                )));
            }
            (core, Some(build))
        }
        None => (input, None),
    };
    let (core, prerelease) = match without_build.split_once('-') {
        Some((core, pre)) => {
            if pre.is_empty() {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{input}` has a malformed pre-release suffix"
                )));
            }
            (core, Some(pre))
        }
        None => (without_build, None),
    };
    let mut parts = core.split('.');
    for _ in 0..3 {
        let part = parts.next().ok_or_else(|| {
            DeliveryError::InvalidArgument(format!("`{input}` is not a semantic version"))
        })?;
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(DeliveryError::InvalidArgument(format!(
                "`{input}` is not a semantic version"
            )));
        }
    }
    if parts.next().is_some() {
        return Err(DeliveryError::InvalidArgument(format!(
            "`{input}` is not a semantic version"
        )));
    }
    if let Some(pre) = prerelease {
        for identifier in pre.split('.') {
            if !valid_identifier(identifier) {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{input}` has a malformed pre-release identifier"
                )));
            }
            if identifier.bytes().all(|byte| byte.is_ascii_digit())
                && identifier.len() > 1
                && identifier.starts_with('0')
            {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{input}` has a numeric pre-release with a leading zero"
                )));
            }
        }
    }
    if let Some(build) = build {
        for identifier in build.split('.') {
            if !valid_identifier(identifier) {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{input}` has a malformed build identifier"
                )));
            }
        }
    }
    Ok(())
}

fn valid_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Orders two version strings by numeric core, then by `SemVer` pre-release rules.
#[must_use]
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let (Ok(left_core), Ok(right_core)) = (Semver::parse(left), Semver::parse(right)) else {
        return left.cmp(right);
    };
    left_core
        .cmp(&right_core)
        .then_with(|| compare_prerelease(prerelease(left), prerelease(right)))
}

fn prerelease(version: &str) -> Option<&str> {
    version
        .split_once('-')
        .map(|(_, suffix)| suffix.split_once('+').map_or(suffix, |(suffix, _)| suffix))
}

fn compare_prerelease(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => {
            let mut left = left.split('.');
            let mut right = right.split('.');
            loop {
                match (left.next(), right.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(left), Some(right)) => {
                        let ordering = compare_prerelease_part(left, right);
                        if ordering != Ordering::Equal {
                            return ordering;
                        }
                    }
                }
            }
        }
    }
}

fn compare_prerelease_part(left: &str, right: &str) -> Ordering {
    let left_numeric = left.bytes().all(|byte| byte.is_ascii_digit());
    let right_numeric = right.bytes().all(|byte| byte.is_ascii_digit());
    match (left_numeric, right_numeric) {
        (true, true) => {
            let left: u64 = left.parse().unwrap_or(u64::MAX);
            let right: u64 = right.parse().unwrap_or(u64::MAX);
            left.cmp(&right)
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => left.cmp(right),
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
    fn orders_prereleases_below_stable_releases() {
        assert!(compare_versions("2.0.0-rc.1", "2.0.0").is_lt());
        assert!(compare_versions("2.0.0", "2.0.0-rc.1").is_gt());
        assert!(compare_versions("2.0.0-rc.2", "2.0.0-rc.10").is_lt());
        assert!(is_prerelease("2.0.0-rc.1"));
        assert!(!is_prerelease("2.0.0"));
        assert!(!is_prerelease("2.0.0+build-1"));
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

    #[test]
    fn strict_semver_rejects_loose_or_malformed_values() {
        for value in [
            "",
            "v0.1.0",
            " 0.1.0",
            "0.1.0 ",
            "0.1",
            "0.1.0.0",
            "01.1.0",
            "0.01.0",
            "0.1.00",
            "0.1.0-",
            "0.1.0-01",
            "0.1.0-rc..1",
            "0.1.0+",
            "0.1.0+build+more",
        ] {
            assert!(validate_strict(value).is_err(), "{value}");
        }
        for value in [
            "0.1.0",
            "7.1.0-beta",
            "1.2.3-rc.1",
            "1.2.3+build.01",
            "1.2.3-rc-1+build-2",
            "0.1.0-rc.1+build.1.2",
        ] {
            validate_strict(value).unwrap();
        }
    }
}
