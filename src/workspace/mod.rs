//! Linked packages — where dependencies live, and how their indexes open (design.md §9,
//! cli.md §6.5).
//!
//! A declared dependency `name` resolves to the directory **`.vaire/packages/<name>`** —
//! a symlink (or real dir) whose target is a package: a directory whose `knowledge.toml`
//! declares that same `name`. The layout is the whole interface: today `vaire add --link`
//! populates it; a future `vaire install` populates the same entries from a shared cache,
//! and nothing here changes.
//!
//! The index stays **federated**: every package carries exactly its own
//! `.vaire/index.db` (same schema), opened lazily through [`PackageHandle::index`]. A
//! standalone package — no `[dependencies]`, no `@pkg/` references — never constructs
//! any of this.

pub mod link;
pub mod resolver;
pub mod satisfy;
pub mod select;

use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::error::{Result, VaireError};
use crate::index::Index;

/// A package's identity: its **declared** manifest `name` (never a directory name).
/// Grows a version component when packages start arriving from a registry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(pub String);

impl PackageId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PackageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One opened package: its canonical root, loaded manifest, and lazily-opened index.
pub struct PackageHandle {
    pub id: PackageId,
    /// Canonicalized package root (symlinks resolved) — one real directory, one handle.
    pub root: PathBuf,
    pub config: Config,
    /// The package the command was invoked from — its index errors keep the exact local
    /// semantics/messages the read commands have always had (exit 4 not-built, exit 3
    /// schema mismatch with the `--full` hint); dependencies get dependency-flavoured
    /// messages instead.
    is_run_root: bool,
    index: OnceCell<Index>,
}

impl PackageHandle {
    /// Open (once) this package's own `.vaire/index.db`, enforcing the schema-version
    /// gate and — when present — the `package_name` meta sanity check. Reads never build:
    /// a missing index is an error naming the fix.
    pub fn index(&self) -> Result<&Index> {
        if self.index.get().is_none() {
            let db = Repo::index_db_at(&self.root);
            if !db.exists() && !self.is_run_root {
                return Err(VaireError::Dependency(format!(
                    "dependency '{}' has no index at {} — run `vaire index` (it builds linked dependencies too)",
                    self.id,
                    db.display()
                )));
            }
            // Missing run-root index → IndexNotBuilt (exit 4), exactly as before M5.
            let index = Index::open(&db)?;
            match index.schema_version() {
                Some(v) if v == crate::index::db::SCHEMA_VERSION => {}
                other => {
                    let found = other.map_or_else(|| "unknown".to_string(), |v| v.to_string());
                    let expects = crate::index::db::SCHEMA_VERSION;
                    let msg = if self.is_run_root {
                        format!(
                            "index schema version {found} is incompatible with this vaire (expects {expects}); \
                             rebuild with `vaire index --full`"
                        )
                    } else {
                        format!(
                            "dependency '{}': index schema version {found} is incompatible (expects {expects}); run `vaire index`",
                            self.id
                        )
                    };
                    return Err(VaireError::IndexCorrupt(msg));
                }
            }
            // `package_name` meta: written since M5; absent (or empty — a build without
            // a named manifest) on older indexes, and then the manifest — which we
            // already validated — is the identity authority.
            if let Some(stored) = index.meta("package_name")?
                && !stored.is_empty()
                && stored != self.id.as_str()
            {
                return Err(VaireError::IndexCorrupt(format!(
                    "index at {} was built for package '{stored}', not '{}'; run `vaire index`",
                    db.display(),
                    self.id
                )));
            }
            let _ = self.index.set(index);
        }
        Ok(self.index.get().expect("just initialized"))
    }
}

/// The linked-package view rooted at the package a command was invoked from.
///
/// Cheap to construct (no scanning — the link model has nothing to discover up front);
/// handles open lazily and are memoized by canonical root, so one real directory reached
/// through different links is one package and dependency cycles terminate structurally.
pub struct Workspace {
    run_root: PathBuf,
    run_root_id: PackageId,
    /// Canonical root → handle. `BTreeMap` for deterministic iteration.
    handles: RefCell<BTreeMap<PathBuf, Rc<PackageHandle>>>,
    /// Rootless only: declared name → the live paths declaring it, from the catalog.
    /// `None` in an ordinary session, which is what keeps author-mode resolution
    /// deterministic — a package's references resolve through *its* declarations and links,
    /// never through whatever happens to be on this machine.
    ///
    /// A `Vec` because two checkouts may declare one name (a fork beside its original, two
    /// worktrees on different branches), and the catalog deliberately reports both rather
    /// than guessing at write time. Resolution refuses the guess too — see
    /// [`Workspace::locate`].
    catalog: Option<BTreeMap<String, Vec<PathBuf>>>,
}

/// The synthetic run-root's package id in a rootless session. Empty deliberately: manifest
/// names are slugs matching `[a-z][a-z0-9-]*`, so no real package can collide with it, and
/// `qualify` therefore marks every node as cross-package — which is exactly right when
/// there is no package you are standing in.
const NO_PACKAGE: &str = "";

impl Workspace {
    /// Build the view for the current (run-root) package. `config` is the already-loaded
    /// manifest from [`crate::commands::Ctx`].
    pub fn new(repo: &Repo, config: &Config) -> Result<Workspace> {
        let run_root = canonical(repo.root())?;
        let run_root_id = PackageId(config.name.clone());
        let ws = Workspace {
            run_root: run_root.clone(),
            run_root_id: run_root_id.clone(),
            handles: RefCell::new(BTreeMap::new()),
            catalog: None,
        };
        ws.handles.borrow_mut().insert(
            run_root.clone(),
            Rc::new(PackageHandle {
                id: run_root_id,
                root: run_root,
                config: config.clone(),
                is_run_root: true,
                index: OnceCell::new(),
            }),
        );
        Ok(ws)
    }

    /// The view for a **rootless** session: no package to stand in, scope taken from the
    /// catalog instead of from a manifest (registry.v2.md §9).
    ///
    /// Modelled as a synthetic run-root that declares every catalogued package as a
    /// dependency, with the catalog supplying where each one lives. That is not a trick to
    /// avoid a second session type — it is the semantics, stated in the one place
    /// resolution already reads:
    ///
    /// * A real package's `@pkg/…` reference still resolves through **its own**
    ///   `[dependencies]` and its own links first ([`Workspace::locate`]), so a file means
    ///   the same thing here as it does to its author.
    /// * The catalog is consulted only where an ordinary session would have run out of
    ///   places to look — never ahead of an author's own wiring.
    ///
    /// The synthetic root owns no index and is never consulted as a member
    /// ([`Workspace::consult_closure`]); ask it for one and you get the error that names
    /// the real problem, which is that a bare id has no package to be relative to.
    pub fn rootless(packages: Vec<(String, PathBuf)>) -> Workspace {
        let run_root = PathBuf::new();
        let run_root_id = PackageId(NO_PACKAGE.to_string());
        let mut catalog: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for (name, path) in packages {
            // Canonicalized first, because the dedupe below is the whole point and two
            // spellings of one directory would otherwise be reported as an ambiguity the
            // user cannot resolve — `/tmp/x` and `/private/tmp/x` on macOS, or any route
            // through a symlinked home.
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            let paths = catalog.entry(name).or_default();
            // Idempotent by path: the same checkout arriving twice (a catalogued package
            // that is also the one being stood in, under `--all`; a store entry the catalog
            // has also seen) is one candidate, not an ambiguity.
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        // Declaring every catalogued name is what lets `step_into` walk out of the
        // synthetic root: the declaration check is the reader's entitlement question
        // ("may I see this package?"), and here the answer is everything registered.
        let config = Config {
            name: NO_PACKAGE.to_string(),
            dependencies: catalog
                .keys()
                .map(|n| (n.clone(), "^0".to_string()))
                .collect(),
            ..Config::default()
        };

        let ws = Workspace {
            run_root: run_root.clone(),
            run_root_id: run_root_id.clone(),
            handles: RefCell::new(BTreeMap::new()),
            catalog: Some(catalog),
        };
        ws.handles.borrow_mut().insert(
            run_root.clone(),
            Rc::new(PackageHandle {
                id: run_root_id,
                root: run_root,
                config,
                is_run_root: true,
                index: OnceCell::new(),
            }),
        );
        ws
    }

    /// Whether this session has no package to stand in.
    pub fn is_rootless(&self) -> bool {
        self.catalog.is_some()
    }

    /// Whether `handle` is the rootless session's **synthetic run root** — the one member
    /// that is not a package.
    ///
    /// Every rootless special case keys on this rather than on the session, and the
    /// distinction is the whole design: a rootless workspace also holds *real* packages,
    /// reached by following a reference into one. For those, nothing has changed — their
    /// references are still their author's, and both the resolution rules and the error
    /// advice must stay the ones that hold inside a package. Keying on the session instead
    /// would tell a reader "check `vaire catalog list`" about a reference whose actual
    /// problem is a `[dependencies]` entry the author never wrote.
    pub(crate) fn is_synthetic(&self, handle: &PackageHandle) -> bool {
        self.is_rootless() && handle.is_run_root
    }

    /// The run-root (current) package's handle.
    pub fn current(&self) -> Rc<PackageHandle> {
        self.handles.borrow()[&self.run_root].clone()
    }

    /// The memoized handle for a canonical package root a prior resolution opened.
    pub fn handle_at(&self, root: &Path) -> Option<Rc<PackageHandle>> {
        self.handles.borrow().get(root).cloned()
    }

    /// The member set a fan-out read consults: the run-root plus every locatable closure
    /// member (sorted + deduped by canonical root), and the names of dependencies that
    /// could not be located — one shared definition so the skip/dedup semantics stay
    /// identical across backlinks, search, suggest, and unresolved.
    pub fn consult_closure(&self) -> (Vec<Rc<PackageHandle>>, Vec<String>) {
        // The synthetic rootless root is not a package and owns no index; the members are
        // exactly the catalogued ones its "dependencies" name.
        let mut members = match self.is_rootless() {
            true => Vec::new(),
            false => vec![self.current()],
        };
        let mut skipped = Vec::new();
        // Rootless fan-out stops at the catalog and does **not** transitively expand:
        // a catalogued package's own links may point at packages nobody catalogued, and
        // letting those into search results would make "the scope is the catalog"
        // (cli.md §6.8) untrue in a way no reader could predict — whether a package
        // appeared would depend on whether some *other* package happened to link it.
        // Following a reference into one still works; that is resolution, not scope.
        let reachable = match self.is_rootless() {
            true => self.catalogued(),
            false => self.closure(),
        };
        for (id, entry) in reachable {
            match entry {
                Ok(handle) => members.push(handle),
                Err(_) => skipped.push(id.to_string()),
            }
        }
        members.sort_by(|a, b| a.root.cmp(&b.root));
        members.dedup_by(|a, b| a.root == b.root);
        (members, skipped)
    }

    /// Locate dependency `name` **for** `source`: the source package's own
    /// `.vaire/packages/<name>` first; then the run-root package **itself** (a dependency
    /// cycle back into the package you're standing in needs no link — you're already
    /// there); then the run-root's links (cli.md §6.5 — own links win; the fallback lets
    /// one flat set of links serve the whole closure). Errors are specific: not linked /
    /// broken link / name mismatch / unreadable manifest.
    pub fn locate(&self, source: &PackageHandle, name: &str) -> Result<Rc<PackageHandle>> {
        if name == source.id.as_str() {
            return Err(VaireError::Dependency(format!(
                "package '{name}' cannot depend on itself; bare references are already local"
            )));
        }
        // Both link probes are skipped for the synthetic rootless root, and not merely as
        // an optimization: its `root` is empty, so `packages_dir_at` would yield the
        // *relative* `.vaire/packages` and resolve against the process working directory —
        // letting whatever happens to sit beside the user's shell answer in place of the
        // catalog. A reader's scope must not depend on which directory they ran from.
        if !self.is_synthetic(source) {
            let own = Repo::packages_dir_at(&source.root).join(name);
            if entry_exists(&own) {
                return self.open(name, &own);
            }
            if name == self.run_root_id.as_str() {
                return Ok(self.current());
            }
            let fallback = Repo::packages_dir_at(&self.run_root).join(name);
            if entry_exists(&fallback) {
                return self.open(name, &fallback);
            }
        }
        // Rootless only, and deliberately last: a reader has no manifest of its own to be
        // reproducible against, so it resolves by declared name across what this machine
        // knows. An author's session never reaches here — `catalog` is `None` — because a
        // dependency silently resolving from ambient machine state is exactly how a
        // manifest stops meaning anything.
        //
        // It is consulted for *every* source, not only the synthetic root, and that is the
        // ordinary run-root fallback rather than an exception to it: the catalog is the
        // rootless session's link set, and a run-root's links have always served the whole
        // closure (cli.md §6.5). Own links still win, so a real package's wiring is never
        // overridden — only its gaps are filled, and only in a session that has no
        // manifest to be reproducible against.
        if let Some(catalog) = &self.catalog {
            match catalog.get(name).map(Vec::as_slice) {
                Some([root]) => return self.open(name, root),
                // Two live checkouts declaring one name. Picking would be a guess, and the
                // guess would be invisible — so it is refused here exactly as it is in
                // author mode (`select::prefer_registered`), and for the same reason.
                Some(roots @ [_, _, ..]) => {
                    let paths: Vec<String> =
                        roots.iter().map(|r| r.display().to_string()).collect();
                    return Err(VaireError::Dependency(format!(
                        "'{name}' is declared by more than one package on this machine ({}) — \
                         choosing between them would be a guess; drop the one you do not \
                         want with `vaire catalog rm <path>`",
                        paths.join(", ")
                    )));
                }
                _ => {}
            }
        }
        Err(match self.is_synthetic(source) {
            true => VaireError::Dependency(format!(
                "no package declaring '{name}' is in your catalog — record one with \
                 `vaire catalog add <path>`, or import a tree with `vaire catalog scan <dir>`"
            )),
            false => VaireError::Dependency(format!(
                "dependency '{name}' (declared by '{}') is not linked — run `vaire add {name} --link <path>` in {}",
                source.id,
                display_root(&source.root),
            )),
        })
    }

    /// The catalogued packages the synthetic rootless root names, located but **not
    /// expanded** — one level, which is the whole scope of a rootless session.
    fn catalogued(&self) -> Vec<(PackageId, Result<Rc<PackageHandle>>)> {
        let root = self.current();
        root.config
            .dependencies
            .keys()
            .map(|name| (PackageId(name.clone()), self.locate(&root, name)))
            .collect()
    }

    /// Open (memoized) the package behind a `.vaire/packages/<name>` entry.
    fn open(&self, name: &str, entry: &Path) -> Result<Rc<PackageHandle>> {
        let root = canonical(entry).map_err(|_| {
            VaireError::Dependency(format!(
                "dependency '{name}': broken link {} (target does not exist)",
                entry.display()
            ))
        })?;
        if let Some(handle) = self.handles.borrow().get(&root) {
            // Same real directory reached again (possibly via a different link name):
            // verify the alias matches the package it actually is.
            if handle.id.as_str() != name {
                return Err(VaireError::Dependency(format!(
                    "dependency '{name}': {} is package '{}' (name mismatch)",
                    root.display(),
                    handle.id
                )));
            }
            return Ok(handle.clone());
        }
        let manifest = root.join("knowledge.toml");
        if !manifest.is_file() {
            return Err(VaireError::Dependency(format!(
                "dependency '{name}': {} is not a package (no knowledge.toml)",
                root.display()
            )));
        }
        let config = Config::load(&manifest).map_err(|e| {
            VaireError::Dependency(format!("dependency '{name}': invalid manifest — {e}"))
        })?;
        if config.name != name {
            return Err(VaireError::Dependency(format!(
                "dependency '{name}': linked package at {} declares name '{}' — identity is declared, never path-derived",
                root.display(),
                config.name
            )));
        }
        let handle = Rc::new(PackageHandle {
            id: PackageId(config.name.clone()),
            root: root.clone(),
            config,
            is_run_root: false,
            index: OnceCell::new(),
        });
        self.handles.borrow_mut().insert(root, handle.clone());
        Ok(handle)
    }

    /// The transitive dependency closure of the run-root package (excluding it), in
    /// deterministic (name-sorted) order. Unavailable dependencies are returned as
    /// `Err` entries rather than failing the walk — callers choose tolerance (`vaire
    /// index` warns and skips; point resolution errors).
    ///
    /// A failed locate never *blocks* a name: locate is keyed (source, name), so a name
    /// one member cannot find may still resolve through a later member's own links —
    /// only names no encountered source could locate come back as `Err` (first error
    /// kept, one entry per name).
    pub fn closure(&self) -> Vec<(PackageId, Result<Rc<PackageHandle>>)> {
        let mut located: std::collections::BTreeSet<PackageId> = [self.run_root_id.clone()].into();
        let mut failed: BTreeMap<PackageId, VaireError> = BTreeMap::new();
        let mut out = Vec::new();
        let mut frontier: Vec<Rc<PackageHandle>> = vec![self.current()];
        while let Some(pkg) = frontier.pop() {
            // BTreeMap iteration keeps dependency order deterministic.
            for name in pkg.config.dependencies.keys() {
                let id = PackageId(name.clone());
                if located.contains(&id) {
                    continue;
                }
                match self.locate(&pkg, name) {
                    Ok(handle) => {
                        located.insert(id.clone());
                        failed.remove(&id);
                        frontier.push(handle.clone());
                        out.push((id, Ok(handle)));
                    }
                    Err(e) => {
                        failed.entry(id).or_insert(e);
                    }
                }
            }
        }
        out.extend(failed.into_iter().map(|(id, e)| (id, Err(e))));
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// A `.vaire/packages/<name>` entry exists (symlink — even broken — or dir). A broken
/// symlink is deliberately "exists": it selects the entry so the error can say *broken*
/// rather than *not linked*.
fn entry_exists(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

fn canonical(p: &Path) -> Result<PathBuf> {
    Ok(std::fs::canonicalize(p)?)
}

/// `target` expressed relative to `base` (both absolute), or `None` when they share no
/// common prefix worth walking (then an absolute path is clearer). Component-wise — no
/// filesystem access.
pub(crate) fn relative_to(target: &Path, base: &Path) -> Option<PathBuf> {
    let t: Vec<_> = target.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common <= 1 {
        return None; // only the root in common
    }
    let mut rel = PathBuf::new();
    for _ in common..b.len() {
        rel.push("..");
    }
    for c in &t[common..] {
        rel.push(c);
    }
    Some(rel)
}

/// The clickable, consumer-relative display form of a file in another package —
/// `../acme-core/knowledge/teams/platform.md` — computed live from the canonical roots
/// (never stored; JSON keeps the stable package-relative `path` + `package` field).
pub fn display_path(run_root: &Path, pkg_root: &Path, rel: &str) -> String {
    let target = pkg_root.join(rel);
    match relative_to(&target, run_root) {
        Some(p) => p.display().to_string(),
        None => target.display().to_string(),
    }
}

/// A short human form of a package root for error messages.
fn display_root(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}
