//! `vaire index [--full] [--working-tree] [--re-embed] [--no-deps]` (cli.md §4.1).
//! Maintain command — not on the MCP surface.
//!
//! Since M5 the default run also **ensures linked dependencies** (cli.md §6.5): after the
//! current package builds, every package in the transitive closure gets its own index
//! built/refreshed — with *its* manifest, repo, and commit anchor, written into *its*
//! `.vaire/` (the federated model, design.md §9). Incremental per dependency; a
//! dependency whose index was embedded by a different provider is fully rebuilt so each
//! index's vectors stay homogeneous. Unavailable dependencies warn and are skipped —
//! `vaire check` escalates. `--no-deps` (or an empty `[dependencies]`) skips the pass
//! entirely; `--re-embed` stays current-package-only.

use crate::commands::Ctx;
use crate::corpus::repo::Repo;
use crate::embed::Embedder;
use crate::error::Result;
use crate::index::Index;
use crate::index::build::{self, Mode};
use crate::output::{DepIndexed, IndexRunOutput};

pub fn run(
    ctx: &Ctx,
    full: bool,
    working_tree: bool,
    re_embed: bool,
    no_deps: bool,
) -> Result<IndexRunOutput> {
    let embedder = ctx.embedder()?;
    if re_embed {
        let summary = build::reembed(&ctx.repo, embedder)?;
        return Ok(IndexRunOutput {
            summary,
            dependencies: Vec::new(),
        });
    }
    let mode = if working_tree {
        Mode::WorkingTree
    } else if full {
        Mode::Full
    } else {
        Mode::Incremental
    };
    let summary = build::run(&ctx.repo, &ctx.config, embedder, mode)?;

    let mut dependencies = Vec::new();
    if !no_deps && !ctx.config.dependencies.is_empty() {
        dependencies = ensure_deps(ctx, embedder)?;
    }

    Ok(IndexRunOutput {
        summary,
        dependencies,
    })
}

/// Build/refresh every linked dependency in the closure with its OWN repo, manifest, and
/// commit anchor, then record the consumer's resolution snapshot (`deps_snapshot` meta) —
/// the serialization source for the future lockfile. Locate failures (not linked, broken,
/// mismatch) are tolerated as warning rows; build failures propagate. Shared with
/// `vaire check`, whose resolution lints need commit-fresh dependency indexes.
pub(crate) fn ensure_deps(ctx: &Ctx, embedder: &dyn Embedder) -> Result<Vec<DepIndexed>> {
    // Satisfy links first: a dependency that is merely *declared* is linked from whatever
    // the catalog knows about, so a fresh clone needs no wiring step. Runs before the
    // closure walk below, which then sees the new links.
    if let Some(note) = crate::commands::catalog::migrate_local_packages(
        &crate::userconfig::config_home(),
        ctx.home(),
    ) {
        eprintln!("note: {note}");
    }
    let satisfied = crate::workspace::satisfy::satisfy(&ctx.repo, &ctx.config, ctx.home());
    for warning in &satisfied.warnings {
        eprintln!("warning: {warning}");
    }

    let ws = ctx.workspace()?;
    let mut rows = Vec::new();
    let mut snapshot = Vec::new();

    for (id, entry) in ws.closure() {
        match entry {
            Err(e) => rows.push(DepIndexed {
                name: id.to_string(),
                status: "missing".to_string(),
                nodes: None,
                commit: None,
                // What the locate failure was, plus what discovery found (or did not) for
                // that name — the two halves of "why is this dependency unavailable".
                note: Some(match satisfied.notes.get(id.as_str()) {
                    Some(extra) => format!("{e} — {extra}"),
                    None => e.to_string(),
                }),
                linked: None,
            }),
            Ok(handle) => {
                let mode = dep_mode(&handle.root, embedder);
                let dep_repo = Repo::discover(Some(&handle.root), &handle.root)?;
                let summary = build::run(&dep_repo, &handle.config, embedder, mode)?;
                snapshot.push(serde_json::json!({
                    "name": id.to_string(),
                    "version": handle.config.version,
                    "root": handle.root.display().to_string(),
                    // The consumer's own constraint for its direct dependencies; null
                    // for transitive members (their constraints live in their owners).
                    "constraint": ctx.config.dependencies.get(id.as_str()),
                }));
                rows.push(DepIndexed {
                    name: id.to_string(),
                    status: "indexed".to_string(),
                    nodes: Some(summary.nodes),
                    commit: summary.commit,
                    note: None,
                    linked: satisfied
                        .linked
                        .iter()
                        .find(|l| l.name == id.as_str())
                        .map(|l| l.target.clone()),
                });
            }
        }
    }

    // The consumer's resolution snapshot — what its links resolved to at index time.
    let consumer = Index::open(&ctx.repo.index_db())?;
    consumer.set_meta(
        "deps_snapshot",
        &serde_json::to_string(&snapshot).expect("json array of snapshot entries"),
    )?;

    Ok(rows)
}

/// Pick the build mode for one dependency: incremental normally (a fresh/missing or
/// schema-stale index full-rebuilds inside `build::run`'s own gate), **full** when its
/// existing vectors were produced by a different embedding provider — the content-hash
/// cache keys on section text only, so a provider switch must not mix vector spaces —
/// or when the file cannot even be opened (the index is a disposable cache: corrupt →
/// rebuild, never abort the run).
fn dep_mode(root: &std::path::Path, embedder: &dyn Embedder) -> Mode {
    let db = Repo::index_db_at(root);
    if !db.exists() {
        return Mode::Incremental;
    }
    let provider = {
        match Index::open(&db) {
            Ok(index) => match index.meta("embed_provider") {
                Ok(p) => p,
                Err(_) => return Mode::Full, // unreadable meta — recreate
            },
            Err(_) => return Mode::Full, // unopenable file — recreate
        }
        // index dropped here — build::run opens (or recreates) the file itself
    };
    match provider {
        Some(p) if p != embedder.identity() => Mode::Full,
        _ => Mode::Incremental,
    }
}
