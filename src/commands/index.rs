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
use crate::model::Version;
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
    let satisfied =
        crate::workspace::satisfy::satisfy(&ctx.repo, &ctx.config, ctx.home(), ctx.is_frozen());
    for warning in &satisfied.warnings {
        eprintln!("warning: {warning}");
    }

    let ws = ctx.workspace()?;
    let store = crate::store::Store::at(ctx.home());
    let mut rows = Vec::new();
    let mut snapshot = Vec::new();
    // What the closure settled on, recorded in `knowledge.lock` at the end of the pass.
    let mut locked: Vec<crate::lockfile::Locked> = Vec::new();
    let mut in_closure: Vec<String> = Vec::new();
    // Why the lockfile could not be written truthfully this run, if it could not.
    let mut lock_blocked: Vec<String> = Vec::new();

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
            // A store entry ships its index already built, by this machine, and is sealed
            // read-only. Skipping it is what makes immutability a *behavior* — the `chmod`
            // is only the enforcement — and it is also the honest thing: rebuilding would
            // produce the same index, having first had to break the seal to write it.
            Ok(handle) if store.contains(&handle.root) => {
                in_closure.push(id.to_string());
                // A store entry that cannot describe itself is a **reason not to write a
                // lockfile**, not a row to leave out. Omitting it silently would keep the
                // previous entry's version — or none at all — while the closure reports the
                // dependency as resolved, so the file would name a resolution that never
                // happened.
                match locked_from_store(&store, id.as_str(), &handle.config.version) {
                    Ok(entry) => locked.push(entry),
                    Err(e) => lock_blocked.push(format!("{id}: {e}")),
                }
                rows.push(DepIndexed {
                    name: id.to_string(),
                    status: "store".to_string(),
                    nodes: None,
                    commit: None,
                    note: Some(format!("{} (immutable)", handle.config.version)),
                    linked: satisfied
                        .linked
                        .iter()
                        .find(|l| l.name == id.as_str())
                        .map(|l| l.target.clone()),
                });
                snapshot.push(serde_json::json!({
                    "name": id.to_string(),
                    "version": handle.config.version,
                    "root": handle.root.display().to_string(),
                    "constraint": ctx.config.dependencies.get(id.as_str()),
                }));
            }
            Ok(handle) => {
                in_closure.push(id.to_string());
                // No digest, deliberately: a working copy has no artifact to checksum and
                // can change between two runs, so recording one would be a reproducibility
                // claim the tool cannot keep (registry.v2.md §7).
                locked.push(crate::lockfile::Locked {
                    name: id.to_string(),
                    version: handle
                        .config
                        .version
                        .parse()
                        .unwrap_or(Version::new(0, 0, 0)),
                    source: crate::lockfile::Source::Workspace,
                    registry: None,
                    sha256: None,
                    pinned: false,
                });
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

    // And the committed half of the same fact. The snapshot above is index meta — machine
    // state, rebuilt whenever the index is; `knowledge.lock` is a file someone can commit,
    // read, and reproduce from. The whole closure is recorded, not just the direct
    // dependencies, because reproducing a resolution means reproducing all of it.
    //
    // Never fatal: a lockfile that could not be written is a lost record, and failing
    // `vaire index` over one would be worse than the record's absence.
    //
    // Two things stop the write, and both leave the existing file **exactly** as it is:
    //
    // * A store entry that could not describe itself. A partial lockfile is worse than a
    //   stale one — stale is safe and merely imprecise (within-major substitutability),
    //   while partial names a resolution that did not happen.
    // * A lockfile this vaire will not read: written by a newer one, or recording a digest
    //   that is not a digest. Refusing to *read* such a file and then overwriting it would
    //   defeat the refusal entirely — the whole point of declining to reinterpret a newer
    //   format is that its contents survive to be read by something that can.
    let blocked = match lock_blocked.is_empty() {
        false => Some(format!(
            "a store entry could not be read: {}",
            lock_blocked.join("; ")
        )),
        true => match crate::lockfile::Lockfile::load(ctx.repo.root()) {
            Ok(previous) => {
                let merged =
                    crate::lockfile::Lockfile::merged(previous.as_ref(), locked, &in_closure);
                merged.write(ctx.repo.root()).err().map(|e| e.to_string())
            }
            Err(e) => Some(e.to_string()),
        },
    };
    if let Some(why) = blocked {
        eprintln!(
            "warning: {} was left unchanged — {why}",
            crate::lockfile::FILE_NAME
        );
    }

    Ok(rows)
}

/// The lockfile entry for a closure member that lives in the store, read from the entry's
/// own `source.toml` — the digest is a fact about the artifact, and the store is where that
/// fact is kept.
fn locked_from_store(
    store: &crate::store::Store,
    name: &str,
    version: &str,
) -> Result<crate::lockfile::Locked> {
    let parsed: Version = version.parse().map_err(|_| {
        crate::error::VaireError::Config(format!(
            "the store entry declares version {version:?}, which is not MAJOR.MINOR.PATCH"
        ))
    })?;
    let source = store.source(name, parsed)?;
    Ok(crate::lockfile::Locked {
        name: name.to_string(),
        version: parsed,
        source: crate::lockfile::Source::Registry,
        registry: source.registry,
        sha256: Some(source.artifact_sha256),
        pinned: false,
    })
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
