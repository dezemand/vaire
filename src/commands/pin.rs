//! `vaire pin` / `vaire unpin` — holding one exact version (registry.v2.md §5).
//!
//! Within a major line, substitutability is the protocol's own promise (§3.1), which is
//! what lets 1.4.2 replace 1.4.1 in the store without asking and lets resolution take the
//! highest satisfying version. A pin is the deliberate opt-out of that promise, for the
//! cases where it turns out not to hold: a release that broke you anyway, a result being
//! reproduced, a citation that has to name an exact version.
//!
//! ## What a pin does, in three places
//!
//! * **Resolution** takes the pinned version instead of the highest satisfying one.
//! * **Retention** keeps it when a newer version in the same major line arrives, instead of
//!   replacing it.
//! * **[`clean`](crate::commands::clean)** treats it as a root.
//!
//! ## Two records, because they answer different questions
//!
//! The pin lives in `knowledge.lock`, which is per-consumer and committed — it travels with
//! the package, so a colleague cloning it reproduces the hold. It is *also* flagged on the
//! catalog's `releases` row, because retention and `clean` run over the whole store with no
//! particular consumer in hand and have to know a version is held without being told again.
//! The catalog flag is a cache of the lockfiles, recomputed rather than tracked
//! ([`clean::refresh_pins`](crate::commands::clean::refresh_pins)), since two consumers may
//! pin the same version and one of them releasing it settles nothing.
//!
//! ## A pin selects a release; it does not change which world answers
//!
//! Resolution order is unchanged (§6): an explicit link, then a working copy the catalog
//! knows, then the store. A pin chooses *within the store* — it does not displace a
//! checkout. If it did, cloning a package with a pinned lockfile would silently stop using
//! your own working copy of that dependency, which is the last thing a consumer-side
//! version hold should do. Where a working copy does answer first, the pin is inert and
//! this command says so rather than leaving it to be discovered.
//!
//! ## You can only pin what you have
//!
//! The version must already be in the store. A lockfile entry is a claim about bytes — its
//! digest is read from the artifact those bytes came from — so recording one for a release
//! this machine has never seen would put a reproducibility claim in a committed file on the
//! strength of a version number somebody typed. The refusal names the `vaire pull` that
//! fixes it, which is one command.

use crate::catalog::Catalog;
use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::lockfile::{Locked, Lockfile};
use crate::model::Version;
use crate::output::PinOutput;
use crate::store::Store;

/// Hold `<name>@<version>` for this package.
pub fn pin(ctx: &Ctx, spec: &str) -> Result<PinOutput> {
    let (name, version) = parse_spec(spec)?;
    let home = ctx.home().to_path_buf();
    let store = Store::at(&home);
    let root = consumer(ctx)?;

    // Declared first, because a pin on something this package does not depend on is not a
    // narrower version of anything — it is a statement about a package that plays no part
    // in this closure, and honoring it would put an entry in the lockfile that resolution
    // never consults.
    let constraint = ctx.config.dependencies.get(&name).ok_or_else(|| {
        VaireError::Usage(format!(
            "'{name}' is not a dependency of this package — declare it with \
             `vaire add {name}` before pinning a version of it"
        ))
    })?;
    // A pin outside the declared major line is a contradiction, and the useful moment to
    // say so is now rather than at the next resolution, where the pin would simply appear
    // not to work.
    if !version.satisfies_caret(constraint) {
        return Err(VaireError::Usage(format!(
            "this package declares '{name}' {constraint}, and {version} is not in that major \
             line — widen the dependency first if that is the version you want"
        )));
    }
    if !store.has(&name, version) {
        return Err(VaireError::Usage(format!(
            "the store has no {name} {version} to hold — fetch it first with \
             `vaire pull {name}@{version}`. A pin records the digest of the artifact it \
             holds, so there is nothing honest to write until the bytes are here"
        )));
    }

    // The digest comes from the entry's own `source.toml`, which is what the materialization
    // wrote from the verified artifact — never from a version number.
    let source = store.source(&name, version)?;
    let entry = Locked {
        name: name.clone(),
        version,
        source: crate::lockfile::Source::Registry,
        registry: source.registry,
        sha256: Some(source.artifact_sha256),
        pinned: true,
    };

    let previous = Lockfile::load(&root)?;
    let mut packages: Vec<Locked> = previous
        .map(|lockfile| lockfile.packages)
        .unwrap_or_default()
        .into_iter()
        .filter(|locked| locked.name != name)
        .collect();
    packages.push(entry);
    Lockfile::new(packages).write(&root)?;

    // Registration is what makes this lockfile findable: `clean` collects its roots from the
    // catalog, so a pin in a package the machine has never recorded would protect nothing.
    // Same hook the other maintain commands use, and it stays silent when it succeeds.
    let mut warnings = crate::commands::catalog::register_path(&home, &root, false);
    let displaced_by = {
        let catalog = Catalog::open(&home)?;
        catalog.set_pinned(&name, version, true)?;
        working_copy(&catalog, &store, &root, &name, constraint)
    };

    // A pin that cannot take effect is worth one line now rather than a puzzle later. Said
    // as a note, not an error: the record is still correct and still travels, and on a
    // machine without that checkout it is exactly what resolves.
    if let Some(path) = displaced_by {
        warnings.push(format!(
            "'{name}' resolves to the working copy at {path} here, which outranks the store \
             — the pin is recorded and will apply wherever that checkout is absent"
        ));
    }

    Ok(PinOutput {
        package: name,
        version: Some(version.to_string()),
        pinned: true,
        lockfile: crate::lockfile::path_for(&root).display().to_string(),
        warnings,
    })
}

/// Release the hold on `name`.
pub fn unpin(ctx: &Ctx, name: &str) -> Result<PinOutput> {
    crate::registry::wire::checked_name(name).map_err(VaireError::Usage)?;
    let home = ctx.home().to_path_buf();
    let root = consumer(ctx)?;

    let lockfile = Lockfile::load(&root)?.ok_or_else(|| {
        VaireError::Usage(format!(
            "this package has no {}, so nothing is pinned",
            crate::lockfile::FILE_NAME
        ))
    })?;
    if !lockfile.get(name).is_some_and(|locked| locked.pinned) {
        return Err(VaireError::Usage(format!("'{name}' is not pinned here")));
    }
    let released = lockfile.get(name).map(|locked| locked.version);
    let packages: Vec<Locked> = lockfile
        .packages
        .into_iter()
        .map(|mut locked| {
            if locked.name == name {
                locked.pinned = false;
            }
            locked
        })
        .collect();
    // The entry itself stays: what it records is still what resolved, and dropping it would
    // erase a reproducible answer in order to release a hold on it.
    Lockfile::new(packages).write(&root)?;

    // Recomputed rather than cleared, because another registered package may pin the same
    // version and this one letting go settles nothing about that.
    {
        let catalog = Catalog::open(&home)?;
        crate::commands::clean::refresh_pins(&catalog, &Store::at(&home))?;
    }

    Ok(PinOutput {
        package: name.to_string(),
        version: released.map(|version| version.to_string()),
        pinned: false,
        lockfile: crate::lockfile::path_for(&root).display().to_string(),
        warnings: Vec::new(),
    })
}

/// The package whose lockfile this pin belongs to.
///
/// A pin is a consumer-side statement, so there has to be a consumer. Outside a package the
/// thing that resembles one — holding a version against `clean` on this machine — is a
/// different statement with a different scope, and quietly doing that instead would leave
/// the user believing they had pinned a dependency.
fn consumer(ctx: &Ctx) -> Result<std::path::PathBuf> {
    if ctx.is_rootless() {
        return Err(VaireError::Usage(format!(
            "a pin is recorded in a package's {}, and there is no package here — run this \
             inside the package that depends on it",
            crate::lockfile::FILE_NAME
        )));
    }
    Ok(ctx.repo.root().to_path_buf())
}

/// Where `name` would resolve from a working copy, if it would — the case that makes a pin
/// inert here.
///
/// Asked of the two things that outrank the store, in resolution's own order (§6): an
/// explicit link, then what the catalog would select. Not of the *current* link, because a
/// package that has not been indexed since declaring the dependency has none yet, and the
/// question is what will answer rather than what answered last.
fn working_copy(
    catalog: &Catalog,
    store: &Store,
    root: &std::path::Path,
    name: &str,
    constraint: &str,
) -> Option<String> {
    let linked = crate::corpus::repo::Repo::packages_dir_at(root).join(name);
    if crate::workspace::link::entry_state(&linked) == crate::workspace::link::EntryState::Present
        && let Ok(target) = std::fs::canonicalize(&linked)
        && !store.contains(&target)
    {
        return Some(target.display().to_string());
    }
    match crate::workspace::select::select(catalog, name, constraint) {
        Ok(crate::workspace::select::Selection::One(candidate)) => {
            Some(candidate.path.display().to_string())
        }
        _ => None,
    }
}

/// `acme-core@1.4.2` — both halves required.
///
/// A bare name is refused rather than read as "whatever is current": a pin whose version
/// was chosen by the tool would move the next time something else did, which is the
/// behavior a pin exists to prevent.
fn parse_spec(spec: &str) -> Result<(String, Version)> {
    let Some((name, version)) = spec.rsplit_once('@') else {
        return Err(VaireError::Usage(format!(
            "'{spec}' names no version — a pin holds an exact one, as `{spec}@1.4.2` \
             (`vaire deps` shows what is resolved now)"
        )));
    };
    crate::registry::wire::checked_name(name).map_err(VaireError::Usage)?;
    let version: Version = version.parse().map_err(|_| {
        VaireError::Usage(format!(
            "'{version}' is not a MAJOR.MINOR.PATCH version — a ^MAJOR constraint is what \
             the manifest declares, and a pin is the opposite of a constraint"
        ))
    })?;
    Ok((name.to_string(), version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_spec_demands_an_exact_version() {
        let (name, version) = parse_spec("acme-core@1.4.2").unwrap();
        assert_eq!(name, "acme-core");
        assert_eq!(version, Version::new(1, 4, 2));

        // "Pin whatever is current" would move the next time something else did.
        let e = parse_spec("acme-core").unwrap_err().to_string();
        assert!(e.contains("names no version"), "{e}");

        // A constraint is what the manifest declares; a pin is its opposite.
        assert!(parse_spec("acme-core@^1").is_err());
        // The name is about to become a directory lookup under the user's home.
        assert!(parse_spec("../../etc@1.0.0").is_err());
    }
}
