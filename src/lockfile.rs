//! `knowledge.lock` — what a package actually resolved to (registry.v2.md §7).
//!
//! The manifest says what this package depends on; the lockfile says what answered. It is
//! written by `pull` and by the ensure pass, **never by hand**, and it exists for two jobs:
//! reproducing a resolution somewhere else, and noticing when what a registry serves under
//! a version is no longer what it served before.
//!
//! ## Two kinds of entry, and the difference is the point
//!
//! A dependency resolved from the **store** records its version, the registry it came from,
//! and the artifact's `sha256`. That is reproducible: `pull --locked` can fetch exactly
//! those bytes anywhere and prove it got them.
//!
//! A dependency resolved from a **working copy** records its version and nothing else,
//! because there is nothing else to record — a checkout has no artifact and no digest, and
//! it can change under you between two runs. Writing a checksum for it would be a
//! reproducibility claim the tool cannot keep.
//!
//! So the file makes the two-worlds gap (§6) legible rather than papering over it: read a
//! lockfile and you can see, per dependency, whether the answer it names can be obtained
//! again. `--frozen` is what turns that from something you can see into something enforced.
//!
//! ## A stale lock is safe, merely imprecise
//!
//! Within-major substitutability is the protocol's own promise (§3.1), so a lock naming
//! 1.4.1 while the store holds 1.4.3 costs precision, not correctness. That is what makes
//! it reasonable to commit this file in leaf packages — where the citability claim lives —
//! and to treat it as informational everywhere else, instead of demanding it be refreshed
//! in lockstep with every dependency's release.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Result, VaireError};
use crate::model::Version;

/// The lockfile's own format version. Bumped on any incompatible change; a file from a
/// newer vaire is **refused rather than reinterpreted**, because a lockfile whose meaning
/// is guessed at is worse than no lockfile at all — it would reproduce something nobody
/// asked for while claiming to be exact.
pub const LOCKFILE_VERSION: u32 = 1;

pub const FILE_NAME: &str = "knowledge.lock";

/// Where the lockfile for a package at `root` lives. Beside `knowledge.toml`, committed or
/// not as the package's own policy decides.
pub fn path_for(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lockfile {
    pub lockfile_version: u32,
    /// Sorted by name, so the file's diff reflects what changed about the resolution rather
    /// than the order the closure happened to be walked in.
    #[serde(default, rename = "package")]
    pub packages: Vec<Locked>,
}

/// One dependency, as it resolved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Locked {
    pub name: String,
    pub version: Version,
    pub source: Source,
    /// Which configured registry served it. Local to whoever wrote the file — two people
    /// may know one bucket by two names — so it is a hint for `--locked`, never a
    /// requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// The artifact digest. Present exactly when [`Source::Registry`]; its absence is what
    /// says "this answer cannot be reproduced".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Held against retention and `gc`, and honored by resolution. Written by `vaire pin`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

/// Where a resolved dependency came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A materialized release in the store. Reproducible: the digest names exact bytes.
    Registry,
    /// A working copy — an explicit link, or a catalog candidate. Not reproducible, and the
    /// missing checksum says so.
    Workspace,
}

/// Whether `digest` is the shape a sha256 is written in.
fn is_sha256(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Locked {
    /// Whether this entry names bytes that can be obtained again.
    pub fn reproducible(&self) -> bool {
        self.source == Source::Registry && self.sha256.is_some()
    }
}

impl Lockfile {
    pub fn new(packages: Vec<Locked>) -> Lockfile {
        let mut packages = packages;
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        Lockfile {
            lockfile_version: LOCKFILE_VERSION,
            packages,
        }
    }

    /// Read the lockfile beside `root`, or `None` when there is none.
    ///
    /// A file from a newer vaire is an error, not an absence: silently ignoring it would
    /// have `--locked` report a reproduction it never performed.
    pub fn load(root: &Path) -> Result<Option<Lockfile>> {
        let path = path_for(root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let lockfile: Lockfile = toml::from_str(&text)
            .map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))?;
        if lockfile.lockfile_version > LOCKFILE_VERSION {
            return Err(VaireError::Config(format!(
                "{} was written by a newer vaire (lockfile version {}, this one understands \
                 {LOCKFILE_VERSION}) — run `vaire upgrade`",
                path.display(),
                lockfile.lockfile_version
            )));
        }
        // A digest is checked at the door rather than wherever it is next used. This file is
        // committed and hand-editable, so its `sha256` is untrusted text — and a value that
        // is not a hex digest cannot match anything, so accepting one only defers the
        // failure to somewhere that has to cope with arbitrary bytes.
        for entry in &lockfile.packages {
            if let Some(digest) = &entry.sha256
                && !is_sha256(digest)
            {
                return Err(VaireError::Config(format!(
                    "{}: {} records a sha256 that is not a 64-character hex digest",
                    path.display(),
                    entry.name
                )));
            }
        }
        Ok(Some(lockfile))
    }

    pub fn get(&self, name: &str) -> Option<&Locked> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Write the lockfile beside `root`, or remove it when there is nothing to record.
    ///
    /// A standalone package should not accumulate an empty file it never asked for, and a
    /// package whose last dependency was dropped should not keep a lockfile describing a
    /// closure that no longer exists.
    pub fn write(&self, root: &Path) -> Result<()> {
        let path = path_for(root);
        if self.packages.is_empty() {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            };
        }
        let body = toml::to_string_pretty(self)
            .map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))?;
        std::fs::write(
            &path,
            format!(
                "# {FILE_NAME} — written by `vaire pull` and by indexing. Do not edit by hand.\n\
                 #\n\
                 # An entry with a sha256 came from a registry and can be reproduced exactly.\n\
                 # An entry without one came from a working copy, which has no artifact to\n\
                 # checksum — see `--frozen` if you need every answer to be reproducible.\n\
                 {body}"
            ),
        )?;
        Ok(())
    }

    /// Merge freshly-resolved entries over whatever was recorded before.
    ///
    /// Existing entries survive being absent from `resolved` — a run that could not reach a
    /// dependency must not silently drop the record of what it used to resolve to, since
    /// that record is the thing someone else reproduces from. Only a dependency the manifest
    /// no longer declares is forgotten, which is why `declared` is passed rather than
    /// inferred.
    ///
    /// A pin survives a refresh that does not mention it, for the same reason a pin exists.
    pub fn merged(
        previous: Option<&Lockfile>,
        resolved: Vec<Locked>,
        declared: &[String],
    ) -> Lockfile {
        let mut merged: BTreeMap<String, Locked> = BTreeMap::new();
        if let Some(previous) = previous {
            for entry in &previous.packages {
                merged.insert(entry.name.clone(), entry.clone());
            }
        }
        for mut entry in resolved {
            if let Some(old) = merged.get(&entry.name) {
                entry.pinned = entry.pinned || old.pinned;
            }
            merged.insert(entry.name.clone(), entry);
        }
        merged.retain(|name, _| declared.iter().any(|d| d == name));
        Lockfile::new(merged.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_registry(name: &str, version: &str) -> Locked {
        Locked {
            name: name.into(),
            version: version.parse().unwrap(),
            source: Source::Registry,
            registry: Some("lab".into()),
            sha256: Some("a".repeat(64)),
            pinned: false,
        }
    }

    fn from_workspace(name: &str, version: &str) -> Locked {
        Locked {
            name: name.into(),
            version: version.parse().unwrap(),
            source: Source::Workspace,
            registry: None,
            sha256: None,
            pinned: false,
        }
    }

    #[test]
    fn only_a_registry_entry_claims_to_be_reproducible() {
        assert!(from_registry("acme-core", "1.4.2").reproducible());
        // Not a gap in the format — a checkout has no artifact and no digest, and it can
        // change under you between two runs. Saying so is the honest record.
        assert!(!from_workspace("acme-internal", "0.3.0").reproducible());
    }

    #[test]
    fn a_lockfile_round_trips_and_stays_name_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let lockfile = Lockfile::new(vec![
            from_registry("acme-web", "2.0.0"),
            from_workspace("acme-core", "1.4.2"),
        ]);
        lockfile.write(dir.path()).unwrap();

        let read = Lockfile::load(dir.path()).unwrap().expect("written");
        let names: Vec<&str> = read.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["acme-core", "acme-web"],
            "sorted, so diffs are legible"
        );
        assert_eq!(
            read.get("acme-web"),
            Some(&from_registry("acme-web", "2.0.0"))
        );
        assert!(read.get("acme-core").unwrap().sha256.is_none());
    }

    #[test]
    fn a_lockfile_from_a_newer_vaire_is_refused_rather_than_reinterpreted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            path_for(dir.path()),
            "lockfile_version = 99\n[[package]]\nname = \"acme-core\"\nversion = \"1.0.0\"\n\
             source = \"registry\"\n",
        )
        .unwrap();
        // Guessing would have `--locked` report a reproduction it never performed.
        let e = Lockfile::load(dir.path()).unwrap_err().to_string();
        assert!(e.contains("newer vaire"), "{e}");
    }

    #[test]
    fn a_digest_that_is_not_a_digest_is_refused_at_the_door() {
        let dir = tempfile::tempdir().unwrap();
        // Hand-edited or corrupted. Accepting it would only defer the failure to code that
        // then has to cope with arbitrary bytes — including a multi-byte character in the
        // middle of a message that wants to abbreviate the digest.
        std::fs::write(
            path_for(dir.path()),
            "lockfile_version = 1\n[[package]]\nname = \"acme-core\"\nversion = \"1.0.0\"\n\
             source = \"registry\"\nsha256 = \"ünreadable\"\n",
        )
        .unwrap();
        let Err(e) = Lockfile::load(dir.path()) else {
            panic!("a non-hex digest is not a digest");
        };
        assert!(e.to_string().contains("hex digest"), "{e}");
    }

    #[test]
    fn an_empty_lockfile_is_removed_rather_than_written() {
        let dir = tempfile::tempdir().unwrap();
        Lockfile::new(vec![from_registry("acme-core", "1.0.0")])
            .write(dir.path())
            .unwrap();
        assert!(path_for(dir.path()).is_file());

        // The last dependency was dropped: keeping a file describing a closure that no
        // longer exists would be worse than having none.
        Lockfile::new(Vec::new()).write(dir.path()).unwrap();
        assert!(!path_for(dir.path()).exists());
        assert!(Lockfile::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn a_refresh_keeps_what_it_could_not_re_resolve_and_forgets_what_was_undeclared() {
        let previous = Lockfile::new(vec![
            from_registry("acme-core", "1.4.2"),
            from_registry("acme-web", "2.0.0"),
            from_registry("acme-gone", "1.0.0"),
        ]);
        // This run resolved only one of them — a network blip, an unlinked dependency.
        let merged = Lockfile::merged(
            Some(&previous),
            vec![from_registry("acme-core", "1.5.0")],
            &["acme-core".into(), "acme-web".into()],
        );

        assert_eq!(
            merged.get("acme-core").unwrap().version.to_string(),
            "1.5.0"
        );
        // Kept: the record of what it used to resolve to is the thing someone else
        // reproduces from, and a failed run must not erase it.
        assert!(merged.get("acme-web").is_some());
        // Forgotten: the manifest no longer declares it.
        assert!(merged.get("acme-gone").is_none());
    }

    #[test]
    fn a_pin_survives_a_refresh_that_does_not_mention_it() {
        let mut pinned = from_registry("acme-core", "1.4.2");
        pinned.pinned = true;
        let previous = Lockfile::new(vec![pinned]);

        let merged = Lockfile::merged(
            Some(&previous),
            vec![from_registry("acme-core", "1.4.2")],
            &["acme-core".into()],
        );
        assert!(
            merged.get("acme-core").unwrap().pinned,
            "a pin that a routine refresh could clear would not be a pin"
        );
    }
}
