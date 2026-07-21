//! Cross-package reference resolution (design.md §9, packages.md §5).
//!
//! Resolution is keyed **(source package, dependency name)**: an `@pkg/type:id` in one
//! of acme-core's files resolves through *acme-core's* `[dependencies]` and links —
//! never the querying consumer's — so a file resolves identically regardless of who
//! consumes it. A `superseded_by:` tombstone that points cross-package **re-enters
//! resolution as the tombstone owner's reference** (the same keying, applied to
//! redirects). Supersession keeps the local engine's semantics: a visited set, and on a
//! cycle it stops and *returns* the node where the cycle closed — never an error.
//! Dependency cycles between packages are legal and cost nothing (resolution needs
//! existence, not topological order; handles are memoized).

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use crate::error::{Result, VaireError};
use crate::index::query::{ResolvedNode, frontmatter_view};
use crate::model::id::{NodeId, NodeType};
use crate::workspace::{PackageHandle, PackageId, Workspace};

/// A resolution result: the node plus the package it lives in. `node.id` (and
/// `requested_id`/`superseded_by`) are qualified with `@pkg/` exactly when they live
/// outside the run-root package, so outputs render correctly without re-deriving.
pub struct Resolved {
    pub package: PackageId,
    /// Canonical root of the owning package — callers join `node.path` onto this.
    pub root: PathBuf,
    pub node: ResolvedNode,
}

/// Resolve `id` as a reference written by `source` (packages.md §5).
///
/// - Bare id → `source`'s own index (bare references are always local — §2 invariant).
/// - `@alias/…` → the alias must be in **`source`'s** manifest `[dependencies]`
///   (undeclared → [`VaireError::Dependency`]), then resolves through `source`'s links.
/// - `superseded_by` chains hop packages by re-binding `source` to each tombstone's
///   owner; cycles terminate and return the node where the cycle closed.
pub fn resolve(ws: &Workspace, source: Rc<PackageHandle>, id: &NodeId) -> Result<Resolved> {
    let run_root_id = ws.current().id.clone();
    // The requested id displays exactly as written (its `@pkg/` kept if it had one).
    let requested_qualified = id.clone();

    let mut owner: Rc<PackageHandle> = match id.package() {
        None => source,
        Some(alias) => step_into(ws, &source, alias)?,
    };
    let mut current = bare(id);
    let mut seen: HashSet<(PackageId, String)> = HashSet::new();

    loop {
        let Some(stored) = owner.index()?.stored(&current)? else {
            return Err(VaireError::IdNotFound(
                qualify(&current, &owner, &run_root_id).to_string(),
            ));
        };
        match stored.superseded_by.as_deref().filter(|s| !s.is_empty()) {
            Some(next) if seen.insert((owner.id.clone(), current.to_string())) => {
                let next: NodeId = next
                    .parse()
                    .map_err(|_| VaireError::IdNotFound(next.to_string()))?;
                if let Some(alias) = next.package() {
                    // Rebind to the TOMBSTONE OWNER's dependency set — the redirect is
                    // that package's reference, not the original querier's.
                    let alias = alias.to_string();
                    owner = step_into(ws, &owner, &alias)?;
                }
                current = bare(&next);
            }
            // Terminal node (no redirect, or a redirect cycle we refuse to follow).
            _ => {
                let final_qualified = qualify(&current, &owner, &run_root_id);
                let followed = final_qualified != requested_qualified;
                return Ok(Resolved {
                    package: owner.id.clone(),
                    root: owner.root.clone(),
                    node: ResolvedNode {
                        node_type: NodeType::new(stored.node_type),
                        path: stored.path,
                        frontmatter: frontmatter_view(&stored.frontmatter),
                        requested_id: followed.then(|| requested_qualified.clone()),
                        superseded_by: followed.then(|| final_qualified.clone()),
                        id: final_qualified,
                    },
                });
            }
        }
    }
}

/// Which of `member`'s declared dependency names resolve to `target`. In v0.2 an alias
/// *is* the declared package name, so this is (at most) an identity — but the API exists
/// now so nothing ever queries `edges.to_package` by the target's canonical name
/// directly: when per-consumer versions arrive, only this map changes. It is also the
/// seam a registry-era reverse query ("which packages reference this entity of mine?")
/// lands on.
pub fn aliases_for(member: &PackageHandle, target: &PackageId) -> Vec<String> {
    member
        .config
        .dependencies
        .keys()
        .filter(|name| name.as_str() == target.as_str())
        .cloned()
        .collect()
}

/// Follow `alias` out of `source`: declared in the source's manifest, then located
/// through the source's links (own-first, run-root fallback — cli.md §6.5).
fn step_into(ws: &Workspace, source: &Rc<PackageHandle>, alias: &str) -> Result<Rc<PackageHandle>> {
    if !source.config.dependencies.contains_key(alias) {
        return Err(VaireError::Dependency(format!(
            "package '{alias}' is not a declared dependency of '{}' — see [dependencies] in knowledge.toml (or run `vaire add {alias} --link <path>`)",
            source.id
        )));
    }
    ws.locate(source, alias)
}

/// The within-package form of a reference: the `@pkg/` qualifier stripped.
fn bare(id: &NodeId) -> NodeId {
    let mut id = id.clone();
    id.package = None;
    id
}

/// Qualify `id` for display from the run-root's perspective: nodes outside the run-root
/// package carry their `@pkg/`, nodes inside it stay bare.
fn qualify(id: &NodeId, owner: &PackageHandle, run_root: &PackageId) -> NodeId {
    let mut id = bare(id);
    if owner.id != *run_root {
        id.package = Some(owner.id.0.clone());
    }
    id
}
