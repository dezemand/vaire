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
    index: OnceCell<Index>,
}

impl PackageHandle {
    /// Open (once) this package's own `.vaire/index.db`, enforcing the schema-version
    /// gate and — when present — the `package_name` meta sanity check. Reads never build:
    /// a missing index is an error naming the fix.
    pub fn index(&self) -> Result<&Index> {
        if self.index.get().is_none() {
            let db = Repo::index_db_at(&self.root);
            if !db.exists() {
                return Err(VaireError::Dependency(format!(
                    "dependency '{}' has no index at {} — run `vaire index` (it builds linked dependencies too)",
                    self.id,
                    db.display()
                )));
            }
            let index = Index::open(&db)?;
            match index.schema_version() {
                Some(v) if v == crate::index::db::SCHEMA_VERSION => {}
                other => {
                    return Err(VaireError::IndexCorrupt(format!(
                        "dependency '{}': index schema version {} is incompatible (expects {}); run `vaire index`",
                        self.id,
                        other.map_or_else(|| "unknown".to_string(), |v| v.to_string()),
                        crate::index::db::SCHEMA_VERSION,
                    )));
                }
            }
            // `package_name` meta: written since M5; absent on older v3 indexes (then the
            // manifest — which we already validated — is the identity authority).
            if let Some(stored) = index.meta("package_name")?
                && stored != self.id.as_str()
            {
                return Err(VaireError::IndexCorrupt(format!(
                    "dependency '{}': its index at {} was built for package '{stored}'; run `vaire index`",
                    self.id,
                    db.display()
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
}

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
        };
        ws.handles.borrow_mut().insert(
            run_root.clone(),
            Rc::new(PackageHandle {
                id: run_root_id,
                root: run_root,
                config: config.clone(),
                index: OnceCell::new(),
            }),
        );
        Ok(ws)
    }

    /// The run-root (current) package's handle.
    pub fn current(&self) -> Rc<PackageHandle> {
        self.handles.borrow()[&self.run_root].clone()
    }

    /// Locate dependency `name` **for** `source`: the source package's own
    /// `.vaire/packages/<name>` first, else the run-root's (cli.md §6.5 — own links win;
    /// the fallback lets one flat set of links serve the whole closure). Errors are
    /// specific: not linked / broken link / name mismatch / unreadable manifest.
    pub fn locate(&self, source: &PackageHandle, name: &str) -> Result<Rc<PackageHandle>> {
        if name == source.id.as_str() {
            return Err(VaireError::Dependency(format!(
                "package '{name}' cannot depend on itself; bare references are already local"
            )));
        }
        let own = Repo::packages_dir_at(&source.root).join(name);
        let fallback = Repo::packages_dir_at(&self.run_root).join(name);
        let entry = if entry_exists(&own) {
            own
        } else if entry_exists(&fallback) {
            fallback
        } else {
            return Err(VaireError::Dependency(format!(
                "dependency '{name}' (declared by '{}') is not linked — run `vaire add {name} --link <path>` in {}",
                source.id,
                display_root(&source.root),
            )));
        };
        self.open(name, &entry)
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
            index: OnceCell::new(),
        });
        self.handles.borrow_mut().insert(root, handle.clone());
        Ok(handle)
    }

    /// The transitive dependency closure of the run-root package (excluding it), in
    /// deterministic (BFS, name-sorted) order. Unavailable dependencies are returned as
    /// `Err` entries rather than failing the walk — callers choose tolerance (`vaire
    /// index` warns and skips; point resolution errors).
    pub fn closure(&self) -> Vec<(PackageId, Result<Rc<PackageHandle>>)> {
        let mut out = Vec::new();
        let mut visited: std::collections::BTreeSet<PackageId> = [self.run_root_id.clone()].into();
        let mut frontier: Vec<Rc<PackageHandle>> = vec![self.current()];
        while let Some(pkg) = frontier.pop() {
            // BTreeMap iteration keeps dependency order deterministic.
            for name in pkg.config.dependencies.keys() {
                let id = PackageId(name.clone());
                if !visited.insert(id.clone()) {
                    continue;
                }
                match self.locate(&pkg, name) {
                    Ok(handle) => {
                        frontier.push(handle.clone());
                        out.push((id, Ok(handle)));
                    }
                    Err(e) => out.push((id, Err(e))),
                }
            }
        }
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

/// A short human form of a package root for error messages.
fn display_root(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}
