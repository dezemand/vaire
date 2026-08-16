//! The store — pulled releases, unpacked and read-only (registry.v2.md §5).
//!
//! `~/.vaire/store/<name>/<version>/` holds what `vaire pull` fetched: the package's own
//! files exactly as its release shipped them, plus a `.vaire/index.db` **this machine
//! built**. A store entry is a package directory like any other, which is the point — the
//! resolver links to one the same way it links to a working copy, and every read command
//! above it neither knows nor cares which kind it got.
//!
//! Two things separate it from a checkout, and both are deliberate:
//!
//! * **It is immutable.** Written once, `chmod a-w`, never rebuilt. That is what makes
//!   "answered against acme-core 1.4.2" a claim anyone can check.
//! * **It is disposable.** The remote keeps every published version forever (a yank is a
//!   flag, never a deletion), so local pruning costs nothing — which is why retention can
//!   be as blunt as one slot per major line.
//!
//! ## The shipped index is a claim, never truth
//!
//! An artifact carries a prebuilt `.vaire/index.db`, and materialization **throws it away
//! and rebuilds from the shipped Markdown** with the consumer's own vaire (the 2026-07-30
//! trust amendment, carried forward by amendment 2). This is the load-bearing decision of
//! the whole module. A published index is a file a publisher produced; adopting it would
//! mean every consumer's answers depend on a stranger's build, and a corpus whose index
//! disagrees with its own text has no way to be caught. Rebuilding costs a second per
//! package and removes an entire class of trust question.
//!
//! What *is* adopted from the shipped database is provenance — `last_indexed_commit` and
//! `index_source`, so the entry can still say which commit these files are — because those
//! are facts about the release rather than claims about the graph.
//!
//! ## Containment
//!
//! An artifact is an archive from somewhere else, so unpacking is the one place this crate
//! treats input as hostile: entries that are absolute, that climb out with `..`, or that
//! are links of any kind are refused outright, and under `.vaire/` only `index.db` is
//! accepted. A refusal aborts the whole materialization — a partially-unpacked artifact is
//! never something to reason about.

// The layout is resolution's business and needs nothing beyond the filesystem; unpacking
// an archive is `pack`'s dependency set, so only that half is gated.
#[cfg(feature = "pack")]
pub mod materialize;

use std::path::{Path, PathBuf};

use crate::error::{Result, VaireError};
use crate::model::Version;

/// Where a store entry describes itself. Written last, before the entry is sealed, so its
/// presence is what marks a directory as a *finished* materialization.
pub const SOURCE_FILE: &str = ".vaire/source.toml";

/// The store rooted in a vaire home.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// What a materialized entry says about itself (`.vaire/source.toml`).
///
/// Self-describing on purpose: the catalog's `releases` rows are an index over this, so a
/// lost or corrupt catalog is refilled by walking the store rather than by re-pulling.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub name: String,
    pub version: Version,
    /// The digest of the artifact this was unpacked from — the lockfile's anchor, and what
    /// lets a suspicious entry be re-checked against the registry without a download.
    pub artifact_sha256: String,
    /// Which vaire built the index here. A store entry is never rebuilt, so this is the
    /// only record of what produced it.
    pub materialized_by: String,
    /// Where it came from: the local name of the registry, and its URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub materialized_at: i64,
}

impl Store {
    pub fn at(home: &Path) -> Store {
        Store {
            root: home.join("store"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `name` at `version` lives, whether or not it is there.
    ///
    /// `None` for a name that is not a safe path segment. The store is the one place a
    /// declared name becomes a directory under the user's home, and a name arrives here from
    /// a manifest, a lockfile, or a command line — none of which this crate wrote. The
    /// registry client checks the same grammar before it builds a URL; checking it here too
    /// is not redundancy but the second half of the same invariant, since the store is
    /// consulted *before* any registry is asked.
    pub fn entry(&self, name: &str, version: Version) -> Option<PathBuf> {
        Some(self.package_dir(name)?.join(version.to_string()))
    }

    /// The directory holding every stored version of `name`, or `None` if the name is not
    /// one this store will construct a path from.
    fn package_dir(&self, name: &str) -> Option<PathBuf> {
        crate::registry::wire::checked_name(name).ok()?;
        Some(self.root.join(name))
    }

    /// Whether a **finished** entry is there.
    ///
    /// Keyed on `source.toml` rather than on the directory, because the directory can exist
    /// mid-rename or after an interrupted removal, and half an entry must never be
    /// resolvable.
    pub fn has(&self, name: &str, version: Version) -> bool {
        self.entry(name, version)
            .is_some_and(|entry| entry.join(SOURCE_FILE).is_file())
    }

    /// Whether `path` is inside this store — the test the ensure pass uses to leave an
    /// entry alone.
    pub fn contains(&self, path: &Path) -> bool {
        match (
            std::fs::canonicalize(&self.root),
            std::fs::canonicalize(path),
        ) {
            (Ok(root), Ok(path)) => path.starts_with(root),
            // Either side can be uncanonicalizable — a store nothing has been pulled into
            // yet, an entry just removed. The literal comparison is the honest fallback,
            // and answering `false` outright would let the ensure pass write into an entry
            // it merely failed to recognize.
            _ => path.starts_with(&self.root),
        }
    }

    /// Every finished entry for `name`, lowest version first.
    ///
    /// Read from the filesystem, not from the catalog: the store is the fact and the
    /// catalog is an index over it, so this is what a rebuild of that index reads and what
    /// resolution falls back to when the two disagree.
    pub fn versions(&self, name: &str) -> Vec<Version> {
        let mut versions: Vec<Version> = Vec::new();
        let Some(package_dir) = self.package_dir(name) else {
            return versions;
        };
        let Ok(entries) = std::fs::read_dir(package_dir) else {
            return versions;
        };
        for entry in entries.flatten() {
            let Ok(version) = entry.file_name().to_string_lossy().parse::<Version>() else {
                continue;
            };
            if entry.path().join(SOURCE_FILE).is_file() {
                versions.push(version);
            }
        }
        versions.sort();
        versions
    }

    /// The highest stored version of `name` satisfying `constraint`.
    pub fn satisfying(&self, name: &str, constraint: &str) -> Option<Version> {
        self.versions(name)
            .into_iter()
            .filter(|version| version.satisfies_caret(constraint))
            .max()
    }

    /// Every package with at least one finished entry, as `(name, path)` for its highest
    /// version — the store's contribution to a rootless session's scope (amendment 11).
    pub fn packages(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // The highest, because a store holds at most one entry per major line and a
            // reader asking "what does this machine know" wants the current answer.
            if let Some(version) = self.versions(&name).into_iter().max()
                && let Some(entry) = self.entry(&name, version)
            {
                out.push((name.clone(), entry));
            }
        }
        out.sort();
        out
    }

    /// Read an entry's `source.toml`.
    pub fn source(&self, name: &str, version: Version) -> Result<Source> {
        let path = self
            .entry(name, version)
            .ok_or_else(|| VaireError::Config(format!("'{name}' is not a usable package name")))?
            .join(SOURCE_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))?;
        toml::from_str(&text).map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))
    }

    /// Delete one entry, undoing the read-only seal first.
    ///
    /// Removal is the *only* thing that ever touches a sealed entry, and it happens for two
    /// reasons: retention replacing a version within its major line, and `gc`. A link
    /// pointing at what went heals the way every broken link heals — by re-resolving.
    pub fn remove(&self, name: &str, version: Version) -> Result<()> {
        let Some(entry) = self.entry(name, version) else {
            return Ok(());
        };
        if !entry.exists() {
            return Ok(());
        }
        unseal(&entry)?;
        std::fs::remove_dir_all(&entry)?;
        // Leave no empty `<name>/` behind: it would make `packages()` report a package with
        // nothing in it.
        if let Some(package_dir) = self.package_dir(name) {
            let _ = std::fs::remove_dir(package_dir);
        }
        Ok(())
    }
}

/// Restore write permission throughout a sealed entry so it can be deleted.
///
/// Public because a sealed entry defeats ordinary cleanup — a test fixture, or anything
/// else holding a store in a temporary directory, has to be able to undo the seal before
/// the directory can go.
///
/// Directories first would be enough on most systems — a file cannot be unlinked through a
/// read-only directory — but the files are cleared too, because a read-only *file* also
/// stops `remove_dir_all` on some platforms, and a half-deleted entry is worse than one
/// that is still there.
pub fn unseal(entry: &Path) -> Result<()> {
    for found in walkdir::WalkDir::new(entry).contents_first(false) {
        let Ok(found) = found else { continue };
        let Ok(metadata) = found.metadata() else {
            continue;
        };
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(found.path(), permissions);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().unwrap();
        let store = Store::at(home.path());
        (home, store)
    }

    fn finished(store: &Store, name: &str, version: &str) {
        let version: Version = version.parse().unwrap();
        let entry = store.entry(name, version).expect("a usable name");
        std::fs::create_dir_all(entry.join(".vaire")).unwrap();
        std::fs::write(entry.join(SOURCE_FILE), "").unwrap();
    }

    #[test]
    fn only_a_finished_entry_counts() {
        let (_home, store) = store();
        let unfinished = store
            .entry("acme-core", Version::new(1, 0, 0))
            .expect("a usable name");
        std::fs::create_dir_all(&unfinished).unwrap();
        // A directory with no `source.toml` is an interrupted materialization, and must
        // never be resolvable.
        assert!(!store.has("acme-core", Version::new(1, 0, 0)));
        assert!(store.versions("acme-core").is_empty());

        finished(&store, "acme-core", "1.0.0");
        assert!(store.has("acme-core", Version::new(1, 0, 0)));
    }

    #[test]
    fn the_highest_satisfying_version_wins_within_a_major() {
        let (_home, store) = store();
        for version in ["1.9.0", "1.10.0", "2.0.0"] {
            finished(&store, "acme-core", version);
        }
        // Numeric, not lexical: 1.10.0 is above 1.9.0.
        assert_eq!(
            store.satisfying("acme-core", "^1"),
            Some(Version::new(1, 10, 0))
        );
        assert_eq!(
            store.satisfying("acme-core", "^2"),
            Some(Version::new(2, 0, 0))
        );
        assert_eq!(store.satisfying("acme-core", "^3"), None);
    }

    #[test]
    fn a_name_that_is_not_a_path_segment_yields_no_store_path() {
        let (_home, store) = store();
        // The store is where a declared name becomes a directory under the user's home, so
        // the check belongs here as well as at the registry client — the store is consulted
        // first, before any registry is asked.
        for bad in ["../../etc", "a/b", "..", ".hidden", "Acme-Core", ""] {
            assert!(store.entry(bad, Version::new(1, 0, 0)).is_none(), "{bad:?}");
            assert!(!store.has(bad, Version::new(1, 0, 0)));
            assert!(store.versions(bad).is_empty());
            assert!(store.satisfying(bad, "^1").is_none());
            assert!(store.remove(bad, Version::new(1, 0, 0)).is_ok());
        }
    }

    #[test]
    fn a_removed_entry_leaves_no_empty_package_behind() {
        let (_home, store) = store();
        finished(&store, "acme-core", "1.0.0");
        store.remove("acme-core", Version::new(1, 0, 0)).unwrap();
        assert!(store.packages().is_empty(), "no phantom package");
        // Removing what is not there is not an error: retention and gc both run over sets
        // that may already have been pruned.
        assert!(store.remove("acme-core", Version::new(1, 0, 0)).is_ok());
    }

    #[test]
    fn the_rootless_scope_takes_the_highest_of_each_package() {
        let (_home, store) = store();
        finished(&store, "acme-core", "1.0.0");
        finished(&store, "acme-core", "2.0.0");
        finished(&store, "acme-glossary", "1.2.0");
        let packages = store.packages();
        assert_eq!(packages.len(), 2);
        assert!(packages[0].1.ends_with("acme-core/2.0.0"), "{packages:?}");
    }
}
