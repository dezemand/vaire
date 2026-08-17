//! `vaire clean` — dropping store entries nothing needs (registry.v2.md §5).
//!
//! The store is disposable by design: the remote keeps every published version forever (a
//! yank is a flag, never a deletion), so anything deleted here can be fetched again. That
//! is what lets a sweep be blunt. What it must not be is *surprising*, so the rule is
//! stated in terms of what is kept rather than what goes.
//!
//! ## Three kinds of root
//!
//! * **Locked** — a version some registered workspace's `knowledge.lock` names. The
//!   lockfile is the record somebody reproduces a resolution from, and deleting bytes it
//!   names would make that record a lie about this machine.
//! * **Pinned** — a version some workspace pinned. A pin exists precisely to survive
//!   automatic removal; a sweep that ignored one would make `vaire pin` decorative.
//! * **Requested** — a package somebody asked for by name. `vaire pull acme-core` from
//!   outside a package writes no lockfile, deliberately (there is no resolution to record),
//!   and that is exactly how a rootless reader assembles a corpus. Rooting only what a
//!   lockfile names would delete every package on the machine that was pulled to *read*,
//!   which is the use the rootless session exists for.
//!
//! Everything else is a leftover: a version retention could not remove, a major line no
//! manifest declares any more, a transitive dependency that left every closure.
//!
//! ## Registration is what makes a lockfile findable
//!
//! Roots come from the catalog, so a package this machine has never recorded contributes
//! none — its lockfile is a file nobody knows to read. That is the third time registration
//! pays for itself, and it is why `vaire pin` records the package it is run in.
//!
//! ## An unreadable lockfile stops the sweep
//!
//! A lockfile written by a newer vaire, or carrying a digest that is not one, is
//! [refused rather than reinterpreted](crate::lockfile::Lockfile::load). Refusing to read
//! it and then treating it as rootless would delete exactly the entries it was protecting,
//! which is worse than either reading or ignoring it consistently. So a single unreadable
//! lockfile stops the whole sweep and says which one.

use std::collections::BTreeSet;

use crate::catalog::Catalog;
use crate::error::{Result, VaireError};
use crate::lockfile::Lockfile;
use crate::model::Version;
use crate::output::{CleanOutput, CleanedEntry};
use crate::store::Store;

pub struct Options<'a> {
    /// Stop holding this package: withdraw the standing request recorded by a named pull,
    /// then sweep as usual. Its locked and pinned versions still survive.
    pub package: Option<&'a str>,
    /// Report what would go; delete nothing.
    pub dry_run: bool,
}

/// What the machine is holding on to, and why.
pub(crate) struct Roots {
    /// Versions a registered workspace's lockfile names.
    pub locked: BTreeSet<(String, Version)>,
    /// Versions a registered workspace's lockfile **pins**.
    pub pinned: BTreeSet<(String, Version)>,
    /// Workspaces whose lockfile could not be read. Not an absence of roots — an unknown
    /// set of them.
    pub unreadable: Vec<String>,
}

/// Collect every root a registered workspace contributes.
///
/// Reads one small TOML file per live sighting. Cheap enough to be exact, which matters:
/// this is the set standing between a sweep and somebody's dependencies.
pub(crate) fn roots(catalog: &Catalog) -> Result<Roots> {
    let mut out = Roots {
        locked: BTreeSet::new(),
        pinned: BTreeSet::new(),
        unreadable: Vec::new(),
    };
    for (_, path) in catalog.live_packages()? {
        match Lockfile::load(&path) {
            Ok(None) => {}
            Ok(Some(lockfile)) => {
                for entry in lockfile.packages {
                    if entry.pinned {
                        out.pinned.insert((entry.name.clone(), entry.version));
                    }
                    out.locked.insert((entry.name, entry.version));
                }
            }
            Err(e) => out.unreadable.push(format!("{}: {e}", path.display())),
        }
    }
    Ok(out)
}

/// Bring the catalog's `pinned` flags back in line with what the lockfiles actually say.
///
/// The column is a **cache** of "some registered workspace pins this", kept because
/// retention consults it on a path that has no consumer in hand. Two consumers may pin the
/// same version, so clearing one pin cannot simply clear the flag — the answer is
/// recomputed from every lockfile rather than tracked incrementally, which is both simpler
/// and the only version that cannot drift.
pub(crate) fn refresh_pins(catalog: &Catalog, store: &Store) -> Result<()> {
    let roots = roots(catalog)?;
    // An unreadable lockfile is an unknown set of pins. Clearing flags on that basis would
    // un-hold versions the file may well be holding, so nothing is cleared at all.
    if !roots.unreadable.is_empty() {
        return Ok(());
    }
    for (name, version) in store.entries() {
        let held = roots.pinned.contains(&(name.clone(), version));
        catalog.set_pinned(&name, version, held)?;
    }
    Ok(())
}

/// `home` rather than a [`Ctx`](crate::commands::Ctx): the store is machine-level, so
/// this has to work from anywhere — notably from outside any package, which is where
/// somebody clearing space is most likely to be standing.
pub fn run(home: &std::path::Path, options: Options<'_>) -> Result<CleanOutput> {
    let store = Store::at(home);

    if let Some(package) = options.package {
        crate::registry::wire::checked_name(package).map_err(VaireError::Usage)?;
    }

    let mut warnings = Vec::new();
    // Opened once, and everything read from it up front: the removals that follow touch
    // the filesystem, and Turso's exclusive open-lock makes a handle held across them a
    // machine-wide stall.
    let (roots, requested) = {
        let catalog = Catalog::open(home)?;
        // The refusal comes **before** anything is written. Withdrawing the request first
        // would leave a run that reported "the sweep did not happen" having cleared a
        // retention flag anyway — and the next successful sweep would then take a package
        // the user never named twice.
        let roots = roots(&catalog)?;
        if !roots.unreadable.is_empty() {
            return Err(VaireError::Config(format!(
                "these lockfiles could not be read, and a sweep that treated them as holding \
                 nothing would delete exactly what they hold:\n  {}",
                roots.unreadable.join("\n  ")
            )));
        }
        // Withdrawing the request is the *point* of naming a package, so it happens even on
        // a dry run — otherwise `--dry-run` would report a sweep the real run would not
        // perform. It is also trivially reversible: pull it again.
        if let Some(package) = options.package
            && !options.dry_run
            && catalog.set_requested(package, false)? == 0
        {
            warnings.push(format!(
                "'{package}' is not in the store, so there was no request to withdraw"
            ));
        }
        let requested: BTreeSet<String> = catalog
            .releases()?
            .into_iter()
            .filter(|entry| entry.requested)
            .map(|entry| entry.name)
            .collect();
        (roots, requested)
    };

    let mut removed = Vec::new();
    let mut kept = 0usize;
    for (name, version) in store.entries() {
        let key = (name.clone(), version);
        // Named explicitly, so a dry run still shows what withdrawing the request would
        // free — the answer the user asked for by typing the name.
        let dropped_request = options.package == Some(name.as_str());
        let held = roots.locked.contains(&key)
            || roots.pinned.contains(&key)
            || (requested.contains(&name) && !dropped_request);
        if held {
            kept += 1;
            continue;
        }
        let freed = store
            .entry(&name, version)
            .map(|path| size_of(&path))
            .unwrap_or(0);
        if !options.dry_run
            && let Err(e) = store.remove(&name, version)
        {
            warnings.push(format!("{name} {version} could not be removed ({e})"));
            kept += 1;
            continue;
        }
        removed.push(CleanedEntry {
            package: name,
            version: version.to_string(),
            bytes: freed,
        });
    }

    // The index follows what it indexes, and only after the removals succeeded — a row
    // dropped for an entry still on disk would hide that entry from the next sweep.
    if !options.dry_run && !removed.is_empty() {
        let catalog = Catalog::open(home)?;
        for entry in &removed {
            if let Ok(version) = entry.version.parse::<Version>() {
                let _ = catalog.forget_release(&entry.package, version);
            }
        }
    }

    Ok(CleanOutput {
        store: store.root().display().to_string(),
        removed,
        kept,
        dry_run: options.dry_run,
        warnings,
    })
}

/// Total size of an entry, for reporting what a sweep frees.
///
/// Best-effort: an unreadable file contributes nothing rather than failing the sweep, since
/// this number is a courtesy and the removal is the work.
fn size_of(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}
