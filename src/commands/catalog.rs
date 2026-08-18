//! `vaire catalog add|rm|list|scan` (cli.md §4.8) — managing what this machine knows.
//!
//! A noun group rather than flat verbs, which is what keeps the three "add"s unambiguous:
//! `add` declares a dependency, `catalog add` records a package on this machine, and
//! `registry add` (later) configures a remote.
//!
//! These are the *manual* surface. The catalog is mostly fed ambiently — every `index`,
//! `check`, or `add` records what it touched ([`register_ambient`]) — so the common case
//! needs none of these commands, and dropping a clone somewhere and running `vaire index`
//! still costs no ceremony. What the manual surface adds is the cases ambience cannot
//! reach: a package you have not run anything in yet (`add`), a tree full of them
//! (`scan`), and removal, which is only ever explicit.

use std::path::{Path, PathBuf};

use crate::catalog::{Catalog, Origin, Sighting, State, scan};
use crate::commands::Ctx;
use crate::config::Config;
use crate::error::{Result, VaireError};
use crate::output::{CatalogListOutput, CatalogRecordOutput, CatalogRemoveOutput};

/// `vaire catalog add [path]` — record a package explicitly.
///
/// Corpus-independent (it takes a path, not a `Ctx`), because the whole point is to
/// register something you are not standing in.
pub fn add(home: &Path, path: Option<&Path>) -> Result<CatalogRecordOutput> {
    let target = path
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    let manifest = target.join("knowledge.toml");
    if !manifest.is_file() {
        return Err(VaireError::Config(format!(
            "{} is not a package — no knowledge.toml",
            target.display()
        )));
    }
    // A store entry is a package directory, so nothing about the path itself refuses this
    // — which is exactly why it has to be refused here. A sighting says "observed at a
    // path, and may have changed since"; a sealed release is the opposite claim, and
    // recording one would make a single directory arrive under two identities, one of them
    // outranking the store in resolution (registry.md §4.3).
    let config = Config::load(&manifest)?;
    if crate::store::Store::at(home).contains(&target) {
        return Err(VaireError::Config(format!(
            "{} is a release in the store, not a working copy — the store already records \
             what this machine holds, and a sighting would claim this directory is one \
             somebody edits. Register the checkout you author {} in instead",
            target.display(),
            config.name
        )));
    }
    let catalog = Catalog::open(home)?;
    catalog.record(&target, &config.name, &config.version, Origin::Registered)?;
    let recorded = catalog
        .by_name(&config.name)?
        .into_iter()
        .find(|s| same_path(&s.path, &target));
    Ok(CatalogRecordOutput {
        catalog: catalog.path().display().to_string(),
        recorded: recorded.into_iter().collect(),
        scanned: None,
        unreadable: Vec::new(),
        truncated: false,
    })
}

/// `vaire catalog scan <dir>` — bulk-import a tree of packages.
///
/// The old discovery walk, in the one place it still belongs. It runs once, on request,
/// and records what it finds; nothing walks anything on the resolution path any more.
pub fn scan_dir(home: &Path, dir: &Path) -> Result<CatalogRecordOutput> {
    if !dir.is_dir() {
        return Err(VaireError::Config(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let walk = scan::walk(dir);
    let store = crate::store::Store::at(home);
    let catalog = Catalog::open(home)?;
    let mut recorded = Vec::new();
    for found in &walk.found {
        // Skipped rather than refused: pointing a scan at a directory that happens to
        // contain the vaire home is an ordinary thing to do (`vaire catalog scan ~`), and
        // failing the whole import over it would be useless. A store entry is simply not
        // what this command imports — it is a sealed release the store already records,
        // and a sighting would claim somebody edits it (registry.md §4.3).
        if store.contains(&found.path) {
            continue;
        }
        catalog.record(
            &found.path,
            &found.config.name,
            &found.config.version,
            Origin::Scanned,
        )?;
        recorded.push(Sighting {
            path: found.path.clone(),
            name: found.config.name.clone(),
            version: found.config.version.clone(),
            state: State::Live,
            origin: Origin::Scanned,
            last_seen: Some(crate::clock::now()),
        });
    }
    Ok(CatalogRecordOutput {
        catalog: catalog.path().display().to_string(),
        recorded,
        scanned: Some(dir.display().to_string()),
        unreadable: walk.unreadable,
        truncated: walk.truncated,
    })
}

/// `vaire catalog rm <path|name>` / `--missing`.
///
/// The argument is a path if it looks like one on disk, otherwise a package name — which
/// removes every sighting declaring it. Removal is the one thing the catalog never does on
/// its own, so this is the only way a row leaves.
pub fn remove(home: &Path, target: Option<&str>, missing: bool) -> Result<CatalogRemoveOutput> {
    let catalog = Catalog::open(home)?;
    if missing {
        // Mark first: a row is only known missing once something has looked.
        catalog.refresh()?;
        let removed = catalog.forget_missing()?;
        return Ok(CatalogRemoveOutput {
            catalog: catalog.path().display().to_string(),
            removed: removed as usize,
            target: "every sighting whose path no longer answers".to_string(),
            swept: true,
        });
    }
    let target = target.ok_or_else(|| {
        VaireError::Usage(
            "`vaire catalog rm` needs a path or a package name (or `--missing`)".into(),
        )
    })?;
    // Try the path first, then the name. Keying off `exists()` would be wrong in exactly
    // the case people reach for this: a checkout that is *gone* is a path you still want
    // removed, and treating it as a name silently matches nothing and reports success.
    let removed = match catalog.forget(&PathBuf::from(target))? {
        true => 1,
        false => catalog.forget_name(target)? as usize,
    };
    Ok(CatalogRemoveOutput {
        catalog: catalog.path().display().to_string(),
        removed,
        target: target.to_string(),
        swept: false,
    })
}

/// `vaire catalog list` — every sighting, states refreshed.
///
/// Refreshing here is a deliberate exception to "reads do not mutate": it writes only to
/// the machine-global catalog, never to a package checkout, and a listing that reported
/// paths as live without checking would be the one thing this command must not be.
pub fn list(home: &Path) -> Result<CatalogListOutput> {
    let catalog = Catalog::open(home)?;
    catalog.refresh()?;
    Ok(CatalogListOutput {
        catalog: catalog.path().display().to_string(),
        sightings: catalog.sightings()?,
    })
}

/// Record this package, and every working copy its dependency closure reached, as a side
/// effect of a maintain command.
///
/// This is what keeps registration from becoming ceremony: by the time anyone needs the
/// catalog to resolve something, ordinary use has already filled it. It rides the commands
/// that already write (`index`, `check`, `add`), so it introduces no new mutation point.
///
/// **Never fails the command.** A catalog that cannot be opened — held too long by another
/// process, on a read-only volume — is a degraded convenience, not a reason for `vaire
/// index` to stop. Problems come back as warnings for the caller to print.
///
/// The handle is opened and dropped inside this function, deliberately: holding it across
/// an index build would lock every other vaire process on the machine out of the catalog
/// for the duration.
pub fn register_ambient(ctx: &Ctx, no_register: bool) -> Vec<String> {
    register_ambient_in(&crate::userconfig::vaire_home(), ctx, no_register)
}

/// [`register_ambient`] against an explicit home — the DI seam tests use, so they never
/// have to set a process-global environment variable to stay hermetic.
pub fn register_ambient_in(home: &Path, ctx: &Ctx, no_register: bool) -> Vec<String> {
    if no_register {
        return Vec::new();
    }
    match try_register_ambient(home, ctx) {
        Ok(()) => Vec::new(),
        Err(e) => vec![format!("catalog not updated: {e}")],
    }
}

/// Record one package by path, for the ambient callers that have no `Ctx` — `vaire add`
/// runs before one is built. Same posture as [`register_ambient`]: never fatal.
pub fn register_path(home: &Path, root: &Path, no_register: bool) -> Vec<String> {
    if no_register {
        return Vec::new();
    }
    let recorded = Config::load(&root.join("knowledge.toml")).and_then(|config| {
        Catalog::open(home)?.record(root, &config.name, &config.version, Origin::Ambient)
    });
    match recorded {
        Ok(()) => Vec::new(),
        Err(e) => vec![format!("catalog not updated: {e}")],
    }
}

/// One-shot migration of the retired `local-packages` root into the catalog
/// (cli.md §6.7).
///
/// v0.2.0 kept a configured root and re-walked it on every maintain command. The catalog
/// replaces both halves — ambient registration for what you author, the store for what you
/// consume — so the setting is removed rather than reinterpreted. On the first maintain
/// command after upgrading, whatever that root held is imported once and the key is
/// dropped: the packages stay resolvable, and nothing walks anything again.
///
/// Returns a line to print when it ran. Never fatal, for the same reason ambient
/// registration is not: a failed convenience must not stop `vaire index`.
pub fn migrate_local_packages(config_home: &Path, home: &Path) -> Option<String> {
    let mut user = crate::userconfig::UserConfig::load_from(config_home).ok()?;
    let root = user.packages.local.take()?;

    // Drop the key **first**. If the import fails — an unreadable root, a locked catalog —
    // retrying it on every later command would reintroduce exactly the per-command walk
    // this migration exists to retire. The message says what to re-run by hand.
    let dropped = user.save_to(config_home);
    let imported = match dropped {
        Ok(_) => scan_dir(home, &root),
        Err(e) => {
            return Some(format!(
                "local-packages is retired, but {} could not be rewritten ({e}) — \
                 remove the `[packages] local` key and run `vaire catalog scan {}`",
                config_home.join("config.toml").display(),
                root.display()
            ));
        }
    };
    Some(match imported {
        Ok(out) => format!(
            "local-packages is retired: imported {} package(s) from {} into the catalog \
             (`vaire catalog list` shows them)",
            out.recorded.len(),
            root.display()
        ),
        Err(e) => format!(
            "local-packages is retired, and importing {} failed ({e}) — \
             run `vaire catalog scan {}` when you can",
            root.display(),
            root.display()
        ),
    })
}

fn try_register_ambient(home: &Path, ctx: &Ctx) -> Result<()> {
    let catalog = Catalog::open(home)?;
    catalog.record(
        ctx.repo.root(),
        &ctx.config.name,
        &ctx.config.version,
        Origin::Ambient,
    )?;
    // Closure members that are working copies — directories someone could edit — are
    // exactly the packages a consumer will want resolved by name later.
    //
    // A **store entry is not one of those**, and recording it here would be a category
    // error with visible consequences: a sighting says "a package was observed at a path
    // and may have changed since", which is the opposite of a sealed release, and the
    // duplicate row makes one directory look like two packages declaring one name. Store
    // entries have their own table, written by `pull`.
    let store = crate::store::Store::at(home);
    if let Ok(ws) = ctx.workspace() {
        for (_, entry) in ws.closure() {
            if let Ok(handle) = entry
                && !store.contains(&handle.root)
            {
                let _ = catalog.record(
                    &handle.root,
                    handle.id.as_str(),
                    &handle.config.version,
                    Origin::Ambient,
                );
            }
        }
    }
    Ok(())
}

/// Whether two paths name the same directory, canonicalizing where possible.
fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
