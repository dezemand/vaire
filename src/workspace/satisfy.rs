//! Satisfying declared dependencies from what this machine has (registry.v2.md §6).
//!
//! A manifest declares *what* a package depends on; where that dependency lives is a
//! consumer question, and the catalog is what answers it. For each declared dependency
//! with no `.vaire/packages/<name>` entry, the catalog is asked for a package declaring
//! that name in the constrained major line, and the link is materialized. A fresh clone is
//! still just `vaire index`.
//!
//! What changed in v0.3 is only the *lookup*. v0.2.0 walked a configured root on every
//! maintain command and matched on name alone; the walk now survives solely as
//! `vaire catalog scan`, and selection is version-aware ([`crate::workspace::select`]).
//! Everything downstream is untouched: the endpoint is still `.vaire/packages/<name>`
//! pointing at a directory, which is what keeps this additive rather than a rewrite.
//!
//! Three properties carry over unchanged:
//!
//! * **Matching is by declared name, never by directory name.** A knowledge base nested
//!   inside a bigger repository is found like any other package.
//! * **Ambiguity is never guessed.** What used to be "two packages declare this name" is
//!   now mostly answered by the constraint; what survives it is reported with both paths.
//! * **Only gaps are filled.** A resolvable entry is never rewritten — an explicit link
//!   always wins. A *broken* entry is replaced, so a moved directory heals.
//!
//! This runs only where links may be written — `vaire add`, and the ensure pass of
//! `vaire index` / `vaire check`. Read commands never reach here, so a query can neither
//! mutate the workspace nor take the catalog's lock.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::catalog::Catalog;
use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::model::Version;
use crate::store::Store;
use crate::workspace::Workspace;
use crate::workspace::link::{self, EntryState};
use crate::workspace::select::{self, Selection};

/// A link the pass created.
#[derive(Debug, Clone)]
pub struct LinkedDep {
    pub name: String,
    /// The package's own directory (what the user recognizes), not the stored link value.
    pub target: String,
}

/// The exact versions this package holds, from its own `knowledge.lock`.
///
/// A pin is a **consumer-side** hold, so it is read from the run root's lockfile and applies
/// to the whole closure that lockfile describes — the links it governs all live in the run
/// root's `.vaire/packages/` (see [`satisfy`]).
///
/// It selects *within the store* and nowhere else. Resolution order is untouched: an
/// explicit link, then a working copy the catalog knows, then the store. A pin that
/// displaced a checkout would mean cloning a package with a pinned lockfile silently stopped
/// using your own copy of that dependency, which is the last thing a version hold should do.
#[derive(Debug, Default)]
pub(crate) struct Pins(BTreeMap<String, Version>);

impl Pins {
    /// Read the run root's pins. An unreadable lockfile yields no pins and a warning — the
    /// same posture the rest of the tool takes towards one it refuses to interpret, and the
    /// visible half matters here because resolution would otherwise silently stop honoring
    /// holds the file still records.
    fn load(root: &Path) -> (Pins, Vec<String>) {
        match crate::lockfile::Lockfile::load(root) {
            Ok(None) => (Pins::default(), Vec::new()),
            Ok(Some(lockfile)) => (
                Pins(
                    lockfile
                        .packages
                        .into_iter()
                        .filter(|locked| locked.pinned)
                        .map(|locked| (locked.name, locked.version))
                        .collect(),
                ),
                Vec::new(),
            ),
            Err(e) => (
                Pins::default(),
                vec![format!("pins are not being honored — {e}")],
            ),
        }
    }

    /// The version of `name` this package holds, if it holds one.
    fn held(&self, name: &str) -> Option<Version> {
        self.0.get(name).copied()
    }
}

/// What a pass did, and what it could not do.
#[derive(Debug, Default)]
pub struct Satisfied {
    pub linked: Vec<LinkedDep>,
    /// Per-dependency explanations for names left unsatisfied — merged into the caller's
    /// own reporting.
    pub notes: BTreeMap<String, String>,
    /// Problems with the catalog itself, reported once rather than per dependency.
    pub warnings: Vec<String>,
}

/// Who asked for a dependency, and under what constraint.
struct Demand {
    by: String,
    constraint: String,
}

/// Satisfy every unlinked dependency in `repo`'s closure from the catalog in `home`.
///
/// Links are always materialized in the **run-root's** `.vaire/packages/` — never inside a
/// dependency's directory — which is also where a transitive dependency is looked up when
/// its consumer has no link of its own (the run-root fallback).
///
/// The pass repeats until it stops making progress: a package's own dependencies only
/// become visible once it is linked, so linking `acme-core` in one pass reveals whatever
/// *it* depends on for the next.
///
/// Nothing here is fatal. An unsatisfiable name keeps the caller's existing "not linked"
/// reporting with a note explaining what the catalog had; a catalog that cannot be opened
/// at all degrades to exactly that, one warning and no links.
pub fn satisfy(repo: &Repo, config: &Config, home: &Path, frozen: bool) -> Satisfied {
    let mut out = Satisfied::default();
    // Read once, from the run root: the lockfile does not change under a pass, and a pin is
    // a statement by *this* package about what it will resolve to.
    let (pins, pin_warnings) = Pins::load(repo.root());
    out.warnings.extend(pin_warnings);
    // Cheap to build (it is a path) and consulted only after the catalog has nothing, so
    // the ordinary all-linked pass still touches neither it nor the catalog.
    let store = Store::at(home);
    // Opened on the first name that actually needs looking up, once, and dropped on
    // return. Both halves of that are load-bearing, and for the same reason: Turso takes
    // an **exclusive** lock when a database is opened.
    //
    // * Lazily, because the steady state — every dependency already linked — is the common
    //   case, and it has nothing to ask. Opening regardless would have every `vaire index`
    //   on the machine queue behind every other one to answer no question at all.
    // * Once, because a handle held across the index build that follows would lock every
    //   other vaire process out of the catalog for that whole time.
    let mut catalog: Option<Catalog> = None;

    loop {
        let Ok(ws) = Workspace::new(repo, config) else {
            return out;
        };
        let (members, unmet) = ws.consult_closure();
        if unmet.is_empty() {
            break;
        }
        // Every constraint the closure places on each name, so a dependency several
        // members share is selected against all of their demands rather than the first.
        let demands = demands(&members);

        let mut progress = false;
        for name in unmet {
            if out.notes.contains_key(&name) {
                continue; // already decided this run; the catalog will not change
            }
            // Cheap check first: an entry already there (a real directory, or a link whose
            // target resolves) is nobody's business here. `satisfy_one` re-checks — this
            // only decides whether to consult the catalog at all.
            if link::entry_state(&Repo::packages_dir_at(repo.root()).join(&name))
                == EntryState::Present
            {
                continue;
            }
            let Some(demands) = demands.get(&name) else {
                continue; // unmet but undeclared: nothing to select against
            };
            let constraint = match intersect(demands) {
                Ok(constraint) => constraint,
                Err(conflict) => {
                    out.notes.insert(name, conflict);
                    continue;
                }
            };
            // Under `--frozen` the catalog is not consulted at all: it is the index of
            // *working copies*, and a working copy is precisely what frozen resolution
            // refuses. Skipping it also means the lock is never taken.
            if frozen {
                match satisfy_one(
                    None,
                    &store,
                    &pins,
                    repo.root(),
                    &name,
                    &constraint,
                    &mut out.warnings,
                ) {
                    Outcome::Linked(dep) => {
                        out.linked.push(dep);
                        progress = true;
                    }
                    Outcome::Note(note) => {
                        out.notes.insert(name, note);
                    }
                    Outcome::Skipped => {}
                }
                continue;
            }
            // First name that genuinely needs the catalog: open it now (see above).
            let catalog = match &catalog {
                Some(catalog) => catalog,
                None => match Catalog::open(home) {
                    Ok(opened) => catalog.insert(opened),
                    Err(e) => {
                        out.warnings.push(format!("catalog unavailable: {e}"));
                        return out;
                    }
                },
            };
            match satisfy_one(
                Some(catalog),
                &store,
                &pins,
                repo.root(),
                &name,
                &constraint,
                &mut out.warnings,
            ) {
                Outcome::Linked(dep) => {
                    out.linked.push(dep);
                    progress = true;
                }
                Outcome::Note(note) => {
                    out.notes.insert(name, note);
                }
                Outcome::Skipped => {}
            }
        }
        if !progress {
            break;
        }
    }

    // Conflicts are judged **after** the links settle, over the whole closure, because a
    // package's own demands only become visible once it is linked. Judged during the walk
    // instead, a conflict would be invisible whenever the first constraint seen happened
    // to resolve — the answer would then depend on link order, which is not a property
    // anyone should have to reason about.
    report_conflicts(repo, config, &mut out);
    out
}

/// Every constraint the closure places on each name.
fn demands(
    members: &[std::rc::Rc<crate::workspace::PackageHandle>],
) -> BTreeMap<String, Vec<Demand>> {
    let mut demands: BTreeMap<String, Vec<Demand>> = BTreeMap::new();
    for member in members {
        for (name, constraint) in &member.config.dependencies {
            demands.entry(name.clone()).or_default().push(Demand {
                by: member.id.to_string(),
                constraint: constraint.clone(),
            });
        }
    }
    demands
}

/// Record every name the settled closure constrains incompatibly — and **withdraw any
/// link this pass made for one**.
///
/// Withdrawing matters: linking one directory per name means a conflicted name resolves
/// for whichever declarer happened to be seen first, and a selector that knowingly leaves
/// an unsatisfiable answer wired up would be worse than the version-blind lint it
/// replaced. Only links *this pass* created are withdrawn — an explicit `--link` is the
/// documented escape hatch from exactly this situation, and must survive it.
fn report_conflicts(repo: &Repo, config: &Config, out: &mut Satisfied) {
    let Ok(ws) = Workspace::new(repo, config) else {
        return;
    };
    let (members, _) = ws.consult_closure();
    for (name, demands) in demands(&members) {
        let Err(conflict) = intersect(&demands) else {
            continue;
        };
        if let Some(at) = out.linked.iter().position(|l| l.name == name) {
            let entry = Repo::packages_dir_at(repo.root()).join(&name);
            if link::remove(&entry).is_ok() {
                out.linked.remove(at);
            }
        }
        let conflict = format!("'{name}': {conflict}");
        if !out.warnings.contains(&conflict) {
            out.warnings.push(conflict.clone());
        }
        out.notes.insert(name, conflict);
    }
}

/// Satisfy a single declared name — `vaire add`'s path, where only the dependency just
/// declared is of interest. Returns what [`satisfy`] would have recorded for it.
pub fn satisfy_name(pkg_root: &Path, name: &str, constraint: &str, home: &Path) -> Satisfied {
    let mut out = Satisfied::default();
    if link::entry_state(&Repo::packages_dir_at(pkg_root).join(name)) == EntryState::Present {
        return out;
    }
    let store = Store::at(home);
    let catalog = match Catalog::open(home) {
        Ok(catalog) => catalog,
        Err(e) => {
            out.warnings.push(format!("catalog unavailable: {e}"));
            return out;
        }
    };
    let (pins, pin_warnings) = Pins::load(pkg_root);
    out.warnings.extend(pin_warnings);
    match satisfy_one(
        Some(&catalog),
        &store,
        &pins,
        pkg_root,
        name,
        constraint,
        &mut out.warnings,
    ) {
        Outcome::Linked(dep) => out.linked.push(dep),
        Outcome::Note(note) => {
            out.notes.insert(name.to_string(), note);
        }
        Outcome::Skipped => {}
    }
    out
}

/// The one constraint that satisfies every demand for a name.
///
/// Constraints are majors-only, so there is no solver here and never will be: the demands
/// either name one major line or they are in conflict. Disjoint majors are a **hard
/// error** rather than a pick, because the closure links one directory per name — quietly
/// choosing one demand would link something another member has said it cannot use.
fn intersect(demands: &[Demand]) -> std::result::Result<String, String> {
    let mut majors: BTreeMap<u64, Vec<&Demand>> = BTreeMap::new();
    for demand in demands {
        match demand
            .constraint
            .strip_prefix('^')
            .and_then(|m| m.parse().ok())
        {
            Some(major) => majors.entry(major).or_default().push(demand),
            // Unparseable constraints are rejected by manifest validation; a manifest that
            // slipped one through must not silently drop the demand.
            None => {
                return Err(format!(
                    "'{}' declares the constraint {:?}, which is not the ^MAJOR form",
                    demand.by, demand.constraint
                ));
            }
        }
    }
    match majors.len() {
        1 => Ok(format!(
            "^{}",
            majors.keys().next().expect("exactly one major")
        )),
        _ => {
            let sides: Vec<String> = majors
                .values()
                .flat_map(|demands| demands.iter())
                .map(|d| format!("{} wants {}", d.by, d.constraint))
                .collect();
            Err(format!(
                "incompatible constraints — {}; one directory is linked per package, so \
                 this cannot be satisfied here (link one explicitly to override)",
                sides.join(", ")
            ))
        }
    }
}

/// The store entry to link for `name`: the pinned version if one is held, else the highest
/// satisfying one.
///
/// A pin that cannot be honored — the version was swept, or deleted by hand — falls back
/// rather than failing, because refusing to resolve at all would be a worse answer than
/// resolving to something usable. It is **said**, though: silently resolving to a different
/// version than the one a committed lockfile pins is exactly the drift a pin exists to
/// prevent, and the fix is one `vaire pull` away.
fn stored(
    store: &Store,
    pins: &Pins,
    name: &str,
    constraint: &str,
    warnings: &mut Vec<String>,
) -> Option<PathBuf> {
    if let Some(held) = pins.held(name) {
        if store.has(name, held) && held.satisfies_caret(constraint) {
            return store.entry(name, held);
        }
        warnings.push(match store.has(name, held) {
            // The manifest moved on and the pin did not. Refusing would strand the package;
            // naming both is what lets somebody see which one to change.
            true => format!(
                "'{name}' is pinned to {held}, which no longer satisfies {constraint} — \
                 `vaire unpin {name}`, or widen the dependency"
            ),
            false => format!(
                "'{name}' is pinned to {held}, which is not in the store — \
                 `vaire pull {name}@{held}` to honor the pin"
            ),
        });
    }
    store
        .satisfying(name, constraint)
        .and_then(|version| store.entry(name, version))
}

/// What to say about a name neither the catalog nor the store can satisfy.
///
/// **Never a fetch.** Resolution does not reach the network (§6), so the most useful thing
/// available is the exact command that would fix it — said alongside whatever the catalog
/// did have, since "no package declaring this" and "one, but in the wrong major line" call
/// for different next steps.
fn unsatisfiable(name: &str, constraint: &str, selection: &Selection) -> String {
    let local = select::explain(name, constraint, selection)
        .unwrap_or_else(|| format!("'{name}' could not be resolved"));
    format!("{local}; or fetch it with `vaire pull {name}@{constraint}`")
}

enum Outcome {
    Linked(LinkedDep),
    /// Nothing to do — the entry is already there and resolvable.
    Skipped,
    Note(String),
}

/// Fill (or heal) one `.vaire/packages/<name>` entry under `pkg_root`.
///
/// The catalog first, then the store (registry.v2.md §6). That order is the two-worlds
/// decision made concrete: a working copy is what you author, so it wins over a pulled
/// release of the same name even when the release is newer. The store is the fallback for
/// what you merely *consume*.
fn satisfy_one(
    catalog: Option<&Catalog>,
    store: &Store,
    pins: &Pins,
    pkg_root: &Path,
    name: &str,
    constraint: &str,
    warnings: &mut Vec<String>,
) -> Outcome {
    let entry = Repo::packages_dir_at(pkg_root).join(name);
    match link::entry_state(&entry) {
        // A resolvable link or a real directory is somebody's deliberate choice: an
        // explicit `--link`, or installed content. This only ever fills gaps.
        EntryState::Present => return Outcome::Skipped,
        // Broken: the target is gone (the package moved or was renamed), so re-selecting
        // heals it. The report names the new target either way.
        EntryState::Broken | EntryState::Absent => {}
    }

    let selection = match catalog {
        Some(catalog) => match select::select(catalog, name, constraint) {
            Ok(selection) => selection,
            Err(e) => return Outcome::Note(format!("could not consult the catalog: {e}")),
        },
        // `--frozen`: nothing local is a candidate, so the store is the only answer.
        None => Selection::Unknown,
    };
    let target: PathBuf = match &selection {
        Selection::One(candidate) => candidate.path.clone(),
        // Ambiguity is a decision to report, never one the store may quietly settle: two
        // working copies both satisfying is a question about which one you meant, and
        // answering it with a third thing would be worse than saying so.
        Selection::Ambiguous(_) => {
            return Outcome::Note(
                select::explain(name, constraint, &selection)
                    .unwrap_or_else(|| format!("'{name}' could not be resolved")),
            );
        }
        Selection::Unknown | Selection::Unsatisfied(_) => {
            match stored(store, pins, name, constraint, warnings) {
                Some(entry) => entry,
                None => {
                    return Outcome::Note(unsatisfiable(name, constraint, &selection));
                }
            }
        }
    };

    match link::plan(pkg_root, name, &target).and_then(|plan| {
        link::commit(plan).map_err(|e| crate::workspace::link::LinkError::Entry(e.to_string()))
    }) {
        Ok(_) => Outcome::Linked(LinkedDep {
            name: name.to_string(),
            target: target.display().to_string(),
        }),
        Err(e) => Outcome::Note(format!("could not link {}: {e}", target.display())),
    }
}
