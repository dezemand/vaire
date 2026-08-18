//! Releasing a package — classify, record, tag (registry.md §3).
//!
//! A release is a **publication event**: between releases there is no version to manage,
//! and the number is computed from what changed rather than typed. This module holds the
//! three pieces that makes possible — the tag vocabulary (here), the classifier
//! ([`classify`]), and the release record ([`record`]) — while `commands::release`
//! orchestrates them and owns the git side.

pub mod adopted;
pub mod classify;
pub mod record;
pub mod summary;

use std::path::Path;

use crate::error::Result;
use crate::model::Version;

/// The tag naming a release, chosen **positionally**: a package that *is* its repository
/// tags `v1.4.2`, while one nested in a larger repository qualifies with its name
/// (`acme-core/v1.4.2`) so sibling packages never collide.
///
/// Only the unqualified form is written today — `release` requires the package to be the
/// repository root, the same constraint `pack` carries — but both forms are *read*
/// ([`parse_release_tag`]), so a package tagged by hand, or one that becomes releasable
/// after a repository reshuffle, still finds its baseline.
pub fn tag_name(package: &str, version: Version, qualified: bool) -> String {
    match qualified {
        true => format!("{package}/v{version}"),
        false => format!("v{version}"),
    }
}

/// The version a tag names, if it names one for `package`. Accepts both forms.
pub fn parse_release_tag(tag: &str, package: &str) -> Option<Version> {
    let bare = match tag.split_once('/') {
        // A qualified tag belongs to exactly one package; another package's tags are not
        // this package's history, so they must not become its baseline.
        Some((prefix, rest)) => (prefix == package).then_some(rest)?,
        None => tag,
    };
    bare.strip_prefix('v')?.parse().ok()
}

/// The highest released version of `package`, with the tag that names it.
///
/// Highest, not most-recent: versioning per package is linear, and a tag's creation date
/// is not its place in that order — a back-patch or a re-fetched history would otherwise
/// silently pick the wrong baseline.
pub fn latest_release(repo_root: &Path, package: &str) -> Result<Option<(String, Version)>> {
    Ok(crate::git::tags(repo_root)?
        .into_iter()
        .filter_map(|tag| {
            let version = parse_release_tag(&tag, package)?;
            Some((tag, version))
        })
        .max_by_key(|(_, version)| *version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_tag_forms_parse_and_foreign_qualified_tags_do_not() {
        assert_eq!(
            parse_release_tag("v1.4.2", "acme-core"),
            Some(Version::new(1, 4, 2))
        );
        assert_eq!(
            parse_release_tag("acme-core/v1.4.2", "acme-core"),
            Some(Version::new(1, 4, 2))
        );
        // A sibling package's tag is not this package's history.
        assert_eq!(parse_release_tag("acme-web/v9.0.0", "acme-core"), None);
        for bad in ["1.4.2", "varia", "v1.4", "release-1.4.2", "vX.Y.Z"] {
            assert_eq!(parse_release_tag(bad, "acme-core"), None, "{bad}");
        }
    }

    #[test]
    fn tag_naming_is_positional() {
        let v = Version::new(1, 4, 2);
        assert_eq!(tag_name("acme-core", v, false), "v1.4.2");
        assert_eq!(tag_name("acme-core", v, true), "acme-core/v1.4.2");
    }
}
