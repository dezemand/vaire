//! Satisfying declared dependencies from the packages you already have locally
//! (cli.md §6.3).
//!
//! A manifest declares *what* a package depends on; where that dependency lives on this
//! machine is a consumer setting. With `local-packages` configured, the maintain commands
//! close that gap themselves: for each declared dependency with no
//! `.vaire/packages/<name>` entry, the root is searched for a package **declaring** that
//! name and the link is materialized. A fresh clone becomes `vaire index` — no per-checkout
//! wiring step.
//!
//! Three properties keep this predictable:
//!
//! * **Matching is by declared name, never by directory name.** A knowledge base nested
//!   inside a bigger repo (`<root>/platform-docs/kb`) is found like any other package.
//! * **Ambiguity is never guessed.** Two packages declaring one name (a fork beside its
//!   original) leave the dependency unsatisfied, with both paths reported.
//! * **Only gaps are filled.** An existing, resolvable entry is never rewritten — an
//!   explicit link always wins. A *broken* entry is replaced: it points at nothing, so
//!   re-discovery heals a moved directory instead of failing.
//!
//! Discovery runs only where links may be written — `vaire add` and the ensure pass of
//! `vaire index` / `vaire check`. Read commands never reach this module, so a query can
//! never mutate the workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::workspace::Workspace;
use crate::workspace::link::{self, EntryState};

/// How deep under the root a package may sit. Knowledge bases are often one component of
/// a larger repository (`<root>/<repo>/docs/kb`), so one level is not enough — but this
/// must not turn into a walk of an entire home directory.
const MAX_DEPTH: usize = 4;

/// Directories never worth descending. Dotted names (`.git`, `.vaire`) are skipped
/// separately; these are the heavy build/vendor trees that never contain a package.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "dist", "build", "venv"];

/// The packages found under the local-packages root, indexed by their **declared** name.
pub struct Scan {
    by_name: BTreeMap<String, Vec<PathBuf>>,
    /// Directories holding an unreadable `knowledge.toml` — surfaced so a malformed
    /// manifest doesn't look like an absent package.
    pub unreadable: Vec<String>,
}

/// The outcome of looking one name up in a [`Scan`].
pub enum Found {
    One(PathBuf),
    None,
    /// More than one package declares the name — reported, never guessed between.
    Ambiguous(Vec<PathBuf>),
}

/// Walk `root` for packages. Each directory holding a `knowledge.toml` is a package: its
/// declared name is recorded and the walk does **not** descend into it (packages don't
/// nest). Canonical paths are visited at most once, so a symlink pointing back inside the
/// root terminates and never doubles a package into a false ambiguity.
pub fn scan(root: &Path) -> Scan {
    let mut by_name: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut unreadable = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];

    while let Some((dir, depth)) = stack.pop() {
        let Ok(dir) = std::fs::canonicalize(&dir) else {
            continue;
        };
        if !seen.insert(dir.clone()) {
            continue;
        }
        let manifest = dir.join("knowledge.toml");
        if manifest.is_file() {
            match Config::load(&manifest) {
                Ok(config) => by_name.entry(config.name).or_default().push(dir),
                Err(e) => unreadable.push(format!("{}: {e}", dir.display())),
            }
            continue;
        }
        if depth == MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                continue;
            }
            // `is_dir` follows symlinks — a root of symlinks into checkouts elsewhere is
            // a normal way to keep this directory.
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            }
        }
    }
    // Filesystem order is arbitrary; sort so an ambiguity reports the same way twice.
    for roots in by_name.values_mut() {
        roots.sort();
    }
    Scan {
        by_name,
        unreadable,
    }
}

impl Scan {
    pub fn find(&self, name: &str) -> Found {
        match self.by_name.get(name) {
            None => Found::None,
            Some(roots) if roots.len() == 1 => Found::One(roots[0].clone()),
            Some(roots) => Found::Ambiguous(roots.clone()),
        }
    }
}

/// A link discovery created.
#[derive(Debug, Clone)]
pub struct LinkedDep {
    pub name: String,
    /// The package's own directory (what the user recognizes), not the stored link value.
    pub target: String,
}

/// What a discovery pass did, and what it could not do.
#[derive(Debug, Default)]
pub struct Satisfied {
    pub linked: Vec<LinkedDep>,
    /// Per-dependency explanations for names left unsatisfied (not found, ambiguous, or a
    /// link that failed to write) — merged into the caller's own reporting.
    pub notes: BTreeMap<String, String>,
    /// Problems with the configured root itself, reported once rather than per dependency.
    pub warnings: Vec<String>,
}

/// Satisfy every unlinked dependency in `repo`'s closure from the local-packages root.
///
/// Links are always materialized in the **run-root's** `.vaire/packages/` — never inside a
/// dependency's directory — which is also where a transitive dependency is looked up when
/// its consumer has no link of its own (cli.md §6.5, the run-root fallback).
///
/// The walk repeats until it stops making progress: a package's own dependencies only
/// become visible once it is linked, so linking `acme-core` in one pass reveals whatever
/// *it* depends on for the next. Nothing here is fatal — an unsatisfiable name keeps the
/// caller's existing "not linked" reporting, with a note explaining what the root held.
pub fn satisfy(repo: &Repo, config: &Config, local_root: Option<&Path>) -> Satisfied {
    let mut out = Satisfied::default();
    let Some(root) = local_root else {
        return out;
    };
    if !root.is_dir() {
        out.warnings.push(format!(
            "local-packages root {} does not exist — set it with `vaire configure local-packages <path>`",
            root.display()
        ));
        return out;
    }

    let scan = scan(root);
    for note in &scan.unreadable {
        out.warnings.push(format!(
            "skipped a package under {}: {note}",
            root.display()
        ));
    }

    loop {
        let Ok(ws) = Workspace::new(repo, config) else {
            return out;
        };
        let mut progress = false;
        for (id, entry) in ws.closure() {
            if entry.is_ok() {
                continue;
            }
            let name = id.as_str();
            if out.notes.contains_key(name) {
                continue; // already decided this run; the scan will not change
            }
            match satisfy_one(repo.root(), name, &scan, root) {
                Outcome::Linked(dep) => {
                    out.linked.push(dep);
                    progress = true;
                }
                Outcome::Note(note) => {
                    out.notes.insert(name.to_string(), note);
                }
                Outcome::Skipped => {}
            }
        }
        if !progress {
            break;
        }
    }
    out
}

/// Satisfy a single declared name — `vaire add`'s path, where only the dependency just
/// declared is of interest. Scans on demand; returns what [`satisfy`] would have recorded.
pub fn satisfy_name(pkg_root: &Path, name: &str, local_root: Option<&Path>) -> Satisfied {
    let mut out = Satisfied::default();
    let Some(root) = local_root else {
        return out;
    };
    if !root.is_dir() {
        out.warnings.push(format!(
            "local-packages root {} does not exist",
            root.display()
        ));
        return out;
    }
    match satisfy_one(pkg_root, name, &scan(root), root) {
        Outcome::Linked(dep) => out.linked.push(dep),
        Outcome::Note(note) => {
            out.notes.insert(name.to_string(), note);
        }
        Outcome::Skipped => {}
    }
    out
}

enum Outcome {
    Linked(LinkedDep),
    /// Nothing to do — the entry is already there and resolvable.
    Skipped,
    Note(String),
}

/// Fill (or heal) one `.vaire/packages/<name>` entry under `pkg_root` from the scan.
fn satisfy_one(pkg_root: &Path, name: &str, scan: &Scan, root: &Path) -> Outcome {
    let entry = Repo::packages_dir_at(pkg_root).join(name);
    match link::entry_state(&entry) {
        // A resolvable link or a real directory is somebody's deliberate choice: an
        // explicit `--link`, or installed content. Discovery only ever fills gaps.
        EntryState::Present => return Outcome::Skipped,
        // Broken: the target is gone (the package moved or was renamed), so re-discovery
        // heals it. The report names the new target either way.
        EntryState::Broken | EntryState::Absent => {}
    }

    let target = match scan.find(name) {
        Found::One(target) => target,
        Found::None => {
            return Outcome::Note(format!(
                "no package declaring '{name}' under {}",
                root.display()
            ));
        }
        Found::Ambiguous(paths) => {
            let list: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            return Outcome::Note(format!(
                "'{name}' is declared by more than one package under {} ({}) — link the one you want explicitly",
                root.display(),
                list.join(", ")
            ));
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
