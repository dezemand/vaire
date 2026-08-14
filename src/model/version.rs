//! Package versions — the `MAJOR.MINOR.PATCH` triple, parsed rather than compared as
//! text (manifest.md §2).
//!
//! Versions arrive as strings from two places: a manifest's `version`, and a release
//! tag's name. Both need ordering ("which is the highest satisfying candidate?") and
//! arithmetic ("what is the next MINOR?"), and string comparison answers neither —
//! `"1.10.0" < "1.9.0"` lexically, which is exactly wrong. Lives in the model core so
//! the manifest layer and the (index-only) release machinery share one definition.
//!
//! Deliberately not general semver: no pre-release, no build metadata. The manifest
//! spec permits exactly three numeric components, so anything else is a parse failure
//! rather than something to represent.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// The type release records carry, and the directory they live in, unless a manifest
/// says otherwise. Conventions rather than reserved words — see `Config::release_type`.
/// They live here, in the parse-only core, because the manifest defaults them and the
/// manifest layer must not depend on the index.
pub const DEFAULT_RELEASE_TYPE: &str = "release";
pub const DEFAULT_RELEASE_DIR: &str = "releases";

/// A parsed `MAJOR.MINOR.PATCH`. Ordering is numeric, component by component.
///
/// Serde round-trips through the string form, in both directions: the registry wire
/// contract writes `"version": "1.4.2"`, and reading it back as a [`Version`] rather than a
/// `String` means a malformed version in a published index document is caught at the
/// document boundary instead of somewhere downstream that assumed it had parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

/// The size of a version change (registry.v2.md §3.1). Ordered by severity, so the
/// "highest applicable" rule for a mixed diff is `max()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    /// The version this one becomes under `bump`. MAJOR and MINOR zero the components
    /// below them — `1.4.2` bumped MINOR is `1.5.0`, never `1.5.2`.
    pub fn bumped(self, bump: Bump) -> Version {
        match bump {
            Bump::Major => Version::new(self.major + 1, 0, 0),
            Bump::Minor => Version::new(self.major, self.minor + 1, 0),
            Bump::Patch => Version::new(self.major, self.minor, self.patch + 1),
        }
    }

    /// Whether this version satisfies a `^MAJOR` constraint. The only constraint form
    /// there is (manifest.md §4), and it is read literally: `^0` matches every `0.x`,
    /// exactly as `^1` matches every `1.x`. A 0.x line simply makes a weaker promise;
    /// the tool does not invent a stricter rule for it.
    pub fn satisfies_caret(self, constraint: &str) -> bool {
        matches!(constraint.strip_prefix('^').and_then(|m| m.parse::<u64>().ok()),
            Some(major) if major == self.major)
    }
}

impl FromStr for Version {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Version, ParseVersionError> {
        let mut parts = s.split('.');
        let mut next = || -> Result<u64, ParseVersionError> {
            let part = parts.next().ok_or(ParseVersionError)?;
            // Reject "+1", "1 ", "０" and every other thing `u64::from_str` would
            // otherwise accept or that would round-trip differently.
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseVersionError);
            }
            part.parse::<u64>().map_err(|_| ParseVersionError)
        };
        let version = Version::new(next()?, next()?, next()?);
        match parts.next() {
            None => Ok(version),
            Some(_) => Err(ParseVersionError),
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl From<Version> for String {
    fn from(v: Version) -> String {
        v.to_string()
    }
}

impl TryFrom<String> for Version {
    type Error = ParseVersionError;

    fn try_from(s: String) -> Result<Version, ParseVersionError> {
        s.parse()
    }
}

impl Bump {
    pub fn as_str(self) -> &'static str {
        match self {
            Bump::Major => "major",
            Bump::Minor => "minor",
            Bump::Patch => "patch",
        }
    }
}

impl fmt::Display for Bump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseVersionError;

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected a MAJOR.MINOR.PATCH version")
    }
}

impl std::error::Error for ParseVersionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_numerically_not_lexically() {
        let nine: Version = "1.9.0".parse().unwrap();
        let ten: Version = "1.10.0".parse().unwrap();
        assert!(nine < ten, "1.9.0 must sort below 1.10.0");
        assert!(Version::new(2, 0, 0) > Version::new(1, 999, 999));
    }

    #[test]
    fn parsing_rejects_anything_that_is_not_three_numeric_components() {
        for bad in ["1.2", "1.2.3.4", "1.2.x", "v1.2.3", "1.2.-3", "", "1..3"] {
            assert!(bad.parse::<Version>().is_err(), "{bad} should not parse");
        }
        assert_eq!("0.0.0".parse::<Version>().unwrap(), Version::new(0, 0, 0));
    }

    #[test]
    fn a_bump_zeroes_everything_below_it() {
        let v: Version = "1.4.2".parse().unwrap();
        assert_eq!(v.bumped(Bump::Patch).to_string(), "1.4.3");
        assert_eq!(v.bumped(Bump::Minor).to_string(), "1.5.0");
        assert_eq!(v.bumped(Bump::Major).to_string(), "2.0.0");
    }

    #[test]
    fn caret_zero_is_read_literally_like_any_other_major() {
        let v: Version = "0.4.0".parse().unwrap();
        assert!(v.satisfies_caret("^0"), "^0 matches every 0.x");
        assert!(!v.satisfies_caret("^1"));
        assert!("1.7.7".parse::<Version>().unwrap().satisfies_caret("^1"));
    }

    #[test]
    fn severity_ordering_makes_a_mixed_diff_take_the_highest() {
        assert!(Bump::Major > Bump::Minor && Bump::Minor > Bump::Patch);
        assert_eq!(
            [Bump::Patch, Bump::Major, Bump::Minor]
                .iter()
                .max()
                .copied(),
            Some(Bump::Major)
        );
    }
}
