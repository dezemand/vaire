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
use crate::index::query::{EdgeRow, ResolvedNode, frontmatter_view};
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

/// One row of a cross-package read: the edge (its `id` already qualified for display
/// from the run-root's perspective) plus which member it came from.
pub struct MemberRow {
    /// The member holding the row; `None` = the run-root package.
    pub package: Option<PackageId>,
    /// That member's canonical root (for consumer-relative display paths).
    pub root: PathBuf,
    pub row: EdgeRow,
}

/// Rows merged across members, plus the dependencies that could not be consulted
/// (unlinked / broken / index missing) — surfaced, never silently dropped.
pub struct CrossRows {
    pub rows: Vec<MemberRow>,
    pub skipped: Vec<String>,
}

/// `backlinks <target>` across the dependency closure ∪ the target's owner: who
/// references this node? The owner member contributes its *local* inbound edges; every
/// other member contributes edges whose `to_package` matches one of ITS aliases for the
/// owner (`aliases_for` — never the canonical name directly). LIMIT is pushed down per
/// member, then re-applied after the merge. Like the local query, the target need not
/// exist — inbound edges are facts about the *referencing* files.
pub fn backlinks(
    ws: &Workspace,
    target: &NodeId,
    type_filter: Option<&NodeType>,
    limit: Option<usize>,
) -> Result<CrossRows> {
    let current = ws.current();
    let owner = match target.package() {
        None => current.clone(),
        Some(alias) => step_into(ws, &current, alias)?,
    };
    let bare_target = bare(target);

    // Members to consult: the run-root + every locatable closure member (the owner is
    // among them by construction — it was stepped into from the run-root's manifest).
    let mut members: Vec<Rc<PackageHandle>> = vec![current.clone()];
    let mut skipped: Vec<String> = Vec::new();
    for (id, entry) in ws.closure() {
        match entry {
            Ok(handle) => members.push(handle),
            Err(_) => skipped.push(id.to_string()),
        }
    }
    members.sort_by(|a, b| a.root.cmp(&b.root));
    members.dedup_by(|a, b| a.root == b.root);

    let mut rows: Vec<MemberRow> = Vec::new();
    for member in members {
        let is_run_root = member.root == current.root;
        let index = match member.index() {
            Ok(index) => index,
            // The run-root's own index failing is the classic local error; a dep's is
            // tolerated and surfaced.
            Err(e) if is_run_root => return Err(e),
            Err(_) => {
                skipped.push(member.id.to_string());
                continue;
            }
        };
        let mut collected = Vec::new();
        if member.root == owner.root {
            collected.extend(index.backlinks(&bare_target, type_filter, limit)?);
        } else {
            for alias in aliases_for(&member, &owner.id) {
                collected.extend(index.backlinks_via(
                    &alias,
                    &bare_target.to_string(),
                    type_filter,
                    limit,
                )?);
            }
        }
        let pkg = (!is_run_root).then(|| member.id.clone());
        for mut row in collected {
            if let Some(p) = &pkg {
                row.id = row.id.with_package(p.0.clone());
            }
            rows.push(MemberRow {
                package: pkg.clone(),
                root: member.root.clone(),
                row,
            });
        }
    }

    rows.sort_by(|a, b| a.row.id.cmp(&b.row.id).then(a.row.line.cmp(&b.row.line)));
    if let Some(n) = limit {
        rows.truncate(n);
    }
    skipped.sort();
    skipped.dedup();
    Ok(CrossRows { rows, skipped })
}

/// `refs <start> --depth N`: the outbound BFS, now crossing package boundaries. Each
/// edge's `@alias` resolves through the edge's OWNING member (source-package keying);
/// dedup key is `(package, within-package id)`; dangling targets — including targets in
/// an unavailable dependency — are dropped exactly like local dangling refs (check
/// surfaces them), with unavailable dependencies additionally listed in `skipped`.
pub fn refs(
    ws: &Workspace,
    start: &NodeId,
    depth: u32,
    type_filter: Option<&NodeType>,
) -> Result<CrossRows> {
    let current = ws.current();
    let start_owner = match start.package() {
        None => current.clone(),
        Some(alias) => step_into(ws, &current, alias)?,
    };
    let bare_start = bare(start);

    let mut seen: HashSet<(PackageId, String)> =
        [(start_owner.id.clone(), bare_start.to_string())].into();
    let mut skipped: Vec<String> = Vec::new();
    let mut found: Vec<MemberRow> = Vec::new();
    let mut frontier: Vec<(Rc<PackageHandle>, NodeId)> = vec![(start_owner, bare_start)];

    for dist in 1..=depth {
        let mut next = Vec::new();
        for (owner, node) in &frontier {
            let index = match owner.index() {
                Ok(index) => index,
                // The start owner's index failing at depth 1 is the query failing;
                // deeper members are tolerated.
                Err(e) if dist == 1 => return Err(e),
                Err(_) => {
                    skipped.push(owner.id.to_string());
                    continue;
                }
            };
            for (to, ref_type, line) in index.outbound(node)? {
                let (t_owner, t_bare) = match to.package() {
                    None => (owner.clone(), bare(&to)),
                    Some(alias) => match step_into(ws, owner, alias) {
                        Ok(handle) => (handle, bare(&to)),
                        // Undeclared/unavailable target package: drop like a dangling
                        // ref (check owns escalation), but surface the package name.
                        Err(VaireError::Dependency(_)) => {
                            skipped.push(alias.to_string());
                            continue;
                        }
                        Err(e) => return Err(e),
                    },
                };
                if !seen.insert((t_owner.id.clone(), t_bare.to_string())) {
                    continue;
                }
                let stored = match t_owner.index() {
                    Ok(index) => index.stored(&t_bare)?,
                    Err(VaireError::Dependency(_)) => {
                        skipped.push(t_owner.id.to_string());
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                // Only real nodes are traversable / returned (dangling → dropped).
                let Some(stored) = stored else { continue };
                let pkg = (t_owner.root != current.root).then(|| t_owner.id.clone());
                let mut qualified = t_bare.clone();
                if let Some(p) = &pkg {
                    qualified.package = Some(p.0.clone());
                }
                found.push(MemberRow {
                    package: pkg,
                    root: t_owner.root.clone(),
                    row: EdgeRow {
                        id: qualified,
                        node_type: NodeType::new(stored.node_type),
                        path: stored.path,
                        ref_type,
                        line,
                        distance: dist,
                    },
                });
                next.push((t_owner, t_bare));
            }
        }
        frontier = next;
    }

    if let Some(t) = type_filter {
        found.retain(|r| &r.row.node_type == t);
    }
    found.sort_by(|a, b| {
        a.row
            .distance
            .cmp(&b.row.distance)
            .then(a.row.id.cmp(&b.row.id))
    });
    skipped.sort();
    skipped.dedup();
    Ok(CrossRows {
        rows: found,
        skipped,
    })
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
