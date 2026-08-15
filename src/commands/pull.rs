//! `vaire pull` (cli.md §4.12) — fetching a release into the store (registry.v2.md §5–§6).
//!
//! The counterpart to `push`, and the only command in the tool that reaches the network to
//! *acquire* knowledge. That it is a command at all is the point: resolution **never
//! fetches silently** (§6). A dependency this machine cannot satisfy is reported with the
//! `vaire pull` that would satisfy it, and then somebody decides. A tool that downloaded
//! a package because a manifest mentioned one would make "what is my corpus" unanswerable
//! without a network trace.
//!
//! ## Choosing a version
//!
//! Constraints are majors-only, so there is no solver: the highest non-yanked version in
//! the constrained major line wins. Within a major, substitutability is the protocol's own
//! promise (§3.1) — that is what lets the choice be arithmetic rather than a decision, and
//! what lets the store keep one slot per major line.
//!
//! A yanked version is skipped for a *new* resolution and still fetchable by exact version,
//! because that is the whole difference between a yank and a deletion.
//!
//! ## Choosing a registry
//!
//! Every configured registry is asked, in priority order, and the **first that has a
//! satisfying version wins** — not the highest version across all of them. A registry is a
//! trust boundary as much as a location, so preferring a higher version from a lower-ranked
//! one would let any registry on the list outbid the one you meant to use.
//!
//! `NotFound` moves on and is forgotten. `PullRestricted` moves on and is **remembered**:
//! if nothing else serves the package, that is the answer worth surfacing, hint and all,
//! because being told where to ask is the entire purpose of the restricted state (§8.5).

use std::path::Path;

use crate::catalog::{Catalog, StoreEntry};
use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::model::Version;
use crate::output::{PullOutput, PulledRelease};
use crate::registry::{Registry, RegistryError};
use crate::store::{Store, materialize};

pub struct Options<'a> {
    /// `<name>` or `<name>@<version>`. Default: every declared dependency this machine
    /// cannot already satisfy.
    pub spec: Option<&'a str>,
    pub registry: Option<&'a str>,
    /// Report what would be fetched; write nothing.
    pub dry_run: bool,
}

pub fn run(ctx: &Ctx, options: Options<'_>) -> Result<PullOutput> {
    let home = ctx.home().to_path_buf();
    let store = Store::at(&home);

    let wanted: Vec<(String, Want)> = match options.spec {
        Some(spec) => vec![parse_spec(spec)?],
        None => unsatisfied_dependencies(ctx, &store)?,
    };
    if wanted.is_empty() {
        return Ok(PullOutput {
            pulled: Vec::new(),
            already: Vec::new(),
            failed: Vec::new(),
            dry_run: options.dry_run,
            warnings: vec![],
            store: store.root().display().to_string(),
        });
    }

    // Opened once for the whole run and dropped before anything slow — Turso's exclusive
    // open-lock makes a handle held across a download a machine-wide stall.
    let registries = {
        let catalog = Catalog::open(&home)?;
        let rows = catalog.registries()?;
        match options.registry {
            None => rows,
            Some(named) => rows.into_iter().filter(|r| r.name == named).collect(),
        }
    };
    if registries.is_empty() {
        return Err(VaireError::Usage(match options.registry {
            Some(named) => format!("no registry called '{named}' is configured"),
            None => "no registry is configured — `vaire registry add <name> <url>`".to_string(),
        }));
    }
    // A row this build cannot open is **reported**, never dropped quietly: otherwise the
    // registry the user expected to answer leaves the fan-out invisibly, and the failure
    // they eventually see lists only the ones that did open.
    let mut warnings = Vec::new();
    let mut clients: Vec<(String, String, Box<dyn Registry>)> = Vec::new();
    for row in &registries {
        match crate::commands::registry::open(row) {
            Ok(client) => clients.push((row.name.clone(), row.url.clone(), client)),
            Err(e) => warnings.push(format!("registry '{}' was skipped: {e}", row.name)),
        }
    }
    if clients.is_empty() {
        // The check above tested the configured *rows*; this tests the clients. Without it a
        // run where every registry failed to open would report "no registry serves X —
        // looked in " with an empty list, which describes nothing that happened.
        return Err(VaireError::Registry(format!(
            "no configured registry could be opened:\n  {}",
            warnings.join("\n  ")
        )));
    }

    let mut out = PullOutput {
        pulled: Vec::new(),
        already: Vec::new(),
        failed: Vec::new(),
        dry_run: options.dry_run,
        warnings,
        store: store.root().display().to_string(),
    };

    for (name, want) in wanted {
        match pull_one(ctx, &store, &clients, &name, &want, options.dry_run) {
            Ok(Outcome::Pulled(release, notes)) => {
                out.pulled.push(release);
                out.warnings.extend(notes);
            }
            Ok(Outcome::Already(version)) => out.already.push(format!("{name} {version}")),
            Err(e) => out.failed.push(crate::output::PullFailure {
                package: name,
                reason: e.to_string(),
            }),
        }
    }
    Ok(out)
}

/// What version of a package is wanted.
enum Want {
    /// The highest non-yanked release in this major line.
    Constraint(String),
    /// Exactly this one — including a yanked one, which is what makes a yank a
    /// recommendation rather than a removal.
    Exact(Version),
}

impl Want {
    fn describe(&self) -> String {
        match self {
            Want::Constraint(constraint) => constraint.clone(),
            Want::Exact(version) => version.to_string(),
        }
    }
}

enum Outcome {
    Pulled(PulledRelease, Vec<String>),
    /// The store already holds it.
    Already(Version),
}

fn pull_one(
    ctx: &Ctx,
    store: &Store,
    clients: &[(String, String, Box<dyn Registry>)],
    name: &str,
    want: &Want,
    dry_run: bool,
) -> Result<Outcome> {
    // The registry is asked first, even when the store already holds *a* version in this
    // major line. `vaire pull acme-glossary` means "bring me the current one", and stopping
    // at whatever is cached would make the command unable to do the thing retention exists
    // to support — 1.4.2 replacing 1.4.1 (§5). The no-op case is still a no-op: it is the
    // *chosen* version being present that makes it one.
    //
    // A bare `vaire pull` never reaches here for a dependency the machine can already
    // satisfy — those are filtered out before any registry is opened.
    let (registry_name, registry_url, client, version) = choose(clients, name, want)?;
    if store.has(name, version) {
        return Ok(Outcome::Already(version));
    }
    if dry_run {
        return Ok(Outcome::Pulled(
            PulledRelease {
                package: name.to_string(),
                version: version.to_string(),
                registry: registry_name,
                path: String::new(),
                replaced: Vec::new(),
            },
            Vec::new(),
        ));
    }

    // Downloaded beside the store rather than into it: nothing enters the store that has
    // not been verified and rebuilt, and the store root stays a directory of package names.
    let scratch = ctx.home().join("downloads");
    std::fs::create_dir_all(&scratch)?;
    let downloaded = scratch.join(format!("{name}-{version}-{}.tgz", std::process::id()));
    let _cleanup = RemoveOnDrop(downloaded.clone());
    let artifact = client.fetch(name, version, &downloaded)?;

    // No embedder is a degradation, not a failure: a package you can read lexically is
    // worth more than a pull that refused because a provider was unconfigured.
    let embedder = ctx.embedder().ok();
    let materialized = materialize::materialize(
        store,
        &artifact,
        Some((registry_name.as_str(), registry_url.as_str())),
        embedder,
    )?;

    let mut warnings = materialized.warnings;
    {
        let catalog = Catalog::open(ctx.home())?;
        catalog.record_release(&StoreEntry {
            name: name.to_string(),
            version,
            registry: Some(registry_name.clone()),
            pinned: false,
            last_used: None,
        })?;
    }
    // Retention: one slot per major line (§5). Safe because within-major substitutability
    // is the protocol's own promise, so this is derived from an invariant rather than
    // bolted on — and safe to be blunt about because the remote keeps every version.
    let replaced = retain(store, ctx.home(), name, version, &mut warnings);

    Ok(Outcome::Pulled(
        PulledRelease {
            package: name.to_string(),
            version: version.to_string(),
            registry: registry_name,
            path: materialized.path.display().to_string(),
            replaced,
        },
        warnings,
    ))
}

/// Ask each registry in turn for a version satisfying `want`.
fn choose<'a>(
    clients: &'a [(String, String, Box<dyn Registry>)],
    name: &str,
    want: &Want,
) -> Result<(String, String, &'a dyn Registry, Version)> {
    let mut restricted: Option<RegistryError> = None;
    let mut looked_in = Vec::new();

    for (registry_name, url, client) in clients {
        looked_in.push(registry_name.as_str());
        let releases = match client.versions(name) {
            Ok(releases) => releases,
            // Nothing here. Forget it and ask the next.
            Err(RegistryError::NotFound { .. }) => continue,
            // Listed, not pullable. Keep it: if nothing else serves this package, the hint
            // is the most useful thing anyone can be told.
            Err(e @ RegistryError::PullRestricted { .. }) => {
                restricted.get_or_insert(e);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let chosen = match want {
            // Exact means exact, yanked included — a lockfile pinning a yanked version has
            // to keep reproducing.
            Want::Exact(version) => releases
                .iter()
                .find(|release| release.version == *version)
                .map(|release| release.version),
            Want::Constraint(constraint) => releases
                .iter()
                .filter(|release| !release.yanked && release.version.satisfies_caret(constraint))
                .map(|release| release.version)
                .max(),
        };
        if let Some(version) = chosen {
            return Ok((registry_name.clone(), url.clone(), client.as_ref(), version));
        }
    }

    if let Some(restricted) = restricted {
        return Err(restricted.into());
    }
    Err(VaireError::Registry(format!(
        "no registry serves {name} {} — looked in {}",
        want.describe(),
        looked_in.join(", ")
    )))
}

/// Drop every other stored version in the same major line as `keep`.
///
/// Never fatal: a version that could not be removed is a warning, because the pull it
/// follows has already succeeded and the only cost of a stale sibling is disk. A link
/// pointing at what went heals the way every broken link heals — by re-resolving.
fn retain(
    store: &Store,
    home: &Path,
    name: &str,
    keep: Version,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut replaced = Vec::new();
    let pinned: Vec<Version> = Catalog::open(home)
        .and_then(|catalog| catalog.releases())
        .map(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.name == name && entry.pinned)
                .map(|entry| entry.version)
                .collect()
        })
        .unwrap_or_default();

    for version in store.versions(name) {
        if version == keep || version.major != keep.major {
            continue;
        }
        // A pin is an explicit statement that this exact version must survive.
        if pinned.contains(&version) {
            continue;
        }
        match store.remove(name, version) {
            Ok(()) => {
                let _ = Catalog::open(home).and_then(|c| c.forget_release(name, version));
                replaced.push(version.to_string());
            }
            Err(e) => warnings.push(format!(
                "{name} {version} is superseded by {keep} but could not be removed ({e})"
            )),
        }
    }
    replaced
}

/// `acme-core` or `acme-core@1.4.2`.
fn parse_spec(spec: &str) -> Result<(String, Want)> {
    // Checked at the boundary where a typed name first becomes a path segment under the
    // store. The registry client checks the same grammar, but only once a registry is being
    // asked — and the store is consulted before that.
    let named = |name: &str, want: Want| -> Result<(String, Want)> {
        crate::registry::wire::checked_name(name).map_err(VaireError::Usage)?;
        Ok((name.to_string(), want))
    };
    match spec.rsplit_once('@') {
        None => named(spec, Want::Constraint("^1".to_string())),
        Some((name, version)) => {
            // `name@^1` is the manifest's own spelling and reads as a constraint; a bare
            // triple is an exact version. Accepting both means the two forms someone might
            // reasonably type both work.
            let want = match version.starts_with('^') {
                true => Want::Constraint(version.to_string()),
                false => Want::Exact(version.parse().map_err(|_| {
                    VaireError::Usage(format!(
                        "'{version}' is neither a ^MAJOR constraint nor a MAJOR.MINOR.PATCH version"
                    ))
                })?),
            };
            named(name, want)
        }
    }
}

/// Every declared dependency the machine cannot already satisfy — what a bare `vaire pull`
/// works on.
///
/// The catalog is *not* consulted here, deliberately: a dependency satisfiable from a
/// working copy is already satisfiable, and pulling a published copy of something you have
/// checked out beside you would replace what you are authoring with what you published.
fn unsatisfied_dependencies(ctx: &Ctx, store: &Store) -> Result<Vec<(String, Want)>> {
    let ws = ctx.workspace()?;
    let mut out = Vec::new();
    for (name, constraint) in &ctx.config.dependencies {
        if ws.locate(&ws.current(), name).is_ok() {
            continue;
        }
        if store.satisfying(name, constraint).is_some() {
            continue;
        }
        out.push((name.clone(), Want::Constraint(constraint.clone())));
    }
    Ok(out)
}

struct RemoveOnDrop(std::path::PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_reads_both_forms_people_type() {
        let (name, want) = parse_spec("acme-core").unwrap();
        assert_eq!(name, "acme-core");
        assert!(matches!(want, Want::Constraint(c) if c == "^1"));

        // The manifest's own spelling.
        let (_, want) = parse_spec("acme-core@^2").unwrap();
        assert!(matches!(want, Want::Constraint(c) if c == "^2"));

        // A bare triple is exact — which is how a yanked version is still reachable.
        let (_, want) = parse_spec("acme-core@1.4.2").unwrap();
        assert!(matches!(want, Want::Exact(v) if v == Version::new(1, 4, 2)));

        assert!(parse_spec("acme-core@1.4").is_err());
        // A name is about to become a directory under the user's home; `../../etc` is not
        // one this tool will construct.
        assert!(parse_spec("../../etc@1.0.0").is_err());
    }
}
