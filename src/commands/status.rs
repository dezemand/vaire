//! `vaire status` (cli.md §4.3). The one read-adjacent command that tolerates a
//! missing index: it reports `last_indexed_commit: null` and exits `0`. Since M5 it also
//! reports each linked dependency's state — index freshness, commit lag, and embedding
//! provider (the observability point for "why does vector search skip this dep?").

use std::collections::BTreeMap;

use crate::commands::Ctx;
use crate::corpus::repo::Repo;
use crate::error::Result;
use crate::index::db::{Index, SCHEMA_VERSION, col_i64, col_text};
use crate::output::{DepStatus, EmbeddingCounts, NodeCounts, StatusOutput};

pub fn run(ctx: &Ctx) -> Result<StatusOutput> {
    let repo = &ctx.repo;
    let repo_path = repo.root().display().to_string();
    let index_path = ".vaire/index.db".to_string();

    let dependencies = dependency_statuses(ctx)?;

    // Tolerate a not-yet-built index: report nulls/zeros, exit 0.
    if !repo.index_db().exists() {
        return Ok(StatusOutput {
            repo: repo_path,
            index_path,
            schema_version: None,
            source: None,
            last_indexed_commit: None,
            commits_behind_head: 0,
            nodes: NodeCounts {
                total: 0,
                by_type: BTreeMap::new(),
            },
            edges: 0,
            embeddings: EmbeddingCounts {
                sections: 0,
                cached: 0,
            },
            embed_provider: None,
            dependencies,
            pending_release: None,
        });
    }

    let index = Index::open(&repo.index_db())?;
    let schema_version = index.schema_version();
    let last_indexed_commit = index.meta("last_indexed_commit")?;
    let source = index.meta("index_source")?;
    let embed_provider = index.meta("embed_provider")?;

    let total = index.scalar_i64("SELECT count(*) FROM nodes", ())? as usize;
    let edges = index.scalar_i64("SELECT count(*) FROM edges", ())? as usize;
    let sections = index.scalar_i64("SELECT count(*) FROM embeddings", ())? as usize;

    let mut by_type = BTreeMap::new();
    for (ty, n) in index.query_rows(
        "SELECT type, count(*) FROM nodes GROUP BY type ORDER BY type",
        (),
        |r| Ok((col_text(r, 0)?, col_i64(r, 1)?)),
    )? {
        by_type.insert(ty, n as usize);
    }

    let commits_behind_head = match &last_indexed_commit {
        Some(commit) => crate::git::commits_ahead(repo.root(), commit)?,
        None => 0,
    };

    let pending_release = pending_release(ctx, &index, commits_behind_head);

    Ok(StatusOutput {
        repo: repo_path,
        index_path,
        schema_version,
        source,
        last_indexed_commit,
        commits_behind_head,
        nodes: NodeCounts { total, by_type },
        edges,
        embeddings: EmbeddingCounts {
            sections,
            cached: sections,
        },
        embed_provider,
        dependencies,
        pending_release,
    })
}

/// What `vaire release` would do right now, so a release is never a surprise
/// (registry.md §3.1).
///
/// Wholly best-effort: `status` reports, it never fails, so every step that could go
/// wrong — no git, no tags, an unreadable baseline — simply yields `None`. Two things
/// keep it cheap enough to run unconditionally: the common "nothing new since the last
/// release" case is answered by a commit count without building anything, and the
/// baseline snapshot it does build carries no embeddings.
///
/// It stays silent while the index is behind HEAD. The classification would then describe
/// neither what was released nor what is committed, and `status` is already saying, one
/// line above, that the index needs rebuilding.
fn pending_release(
    ctx: &Ctx,
    index: &Index,
    commits_behind_head: u32,
) -> Option<crate::output::PendingRelease> {
    if commits_behind_head > 0 {
        return None;
    }
    let root = ctx.repo.root();
    let Some((tag, _)) = crate::release::latest_release(root, &ctx.config.name).ok()? else {
        // Never released: the manifest's version is what a first release would publish,
        // and everything indexed is what it would carry.
        return Some(crate::output::PendingRelease::from(
            crate::release::classify::initial(index, &ctx.config.release_type).ok()?,
            None,
        ));
    };

    // The cheap answer first — no commits since the release means no release to compute.
    let oid = crate::git::resolve_rev(root, &tag).ok()??;
    if crate::git::commits_ahead(root, &oid).ok()? == 0 {
        return Some(crate::output::PendingRelease::from(
            crate::release::classify::nothing(),
            Some(tag),
        ));
    }

    let baseline_manifest = crate::git::show_many_at(root, &tag, &["knowledge.toml".to_string()])
        .ok()?
        .pop()?;
    let config = baseline_manifest
        .and_then(|text| {
            crate::config::Config::parse(&text, "knowledge.toml at the last release").ok()
        })
        .unwrap_or_else(|| ctx.config.clone());

    let scratch = Repo::prepare_derived_dir(root)
        .ok()?
        .join(format!(".status-baseline-{}.db", std::process::id()));
    let classification = crate::index::build::snapshot(root, &config, &tag, &scratch)
        .ok()
        .and_then(|()| Index::open(&scratch).ok())
        .and_then(|before| {
            // Each side is excluded by the release type *its own* manifest declared, so a
            // renamed type does not present the old records as removals.
            crate::release::classify::diff(
                &before,
                &config.release_type,
                index,
                &ctx.config.release_type,
            )
            .ok()
        });
    let _ = crate::index::build::remove_db_files(&scratch);

    Some(crate::output::PendingRelease::from(
        classification?,
        Some(tag),
    ))
}

/// One row per declared dependency (transitive closure), fully tolerant — status never
/// fails because a dependency is unlinked, unbuilt, or corrupt; it reports that instead.
fn dependency_statuses(ctx: &Ctx) -> Result<Vec<DepStatus>> {
    if ctx.config.dependencies.is_empty() {
        return Ok(Vec::new());
    }
    let ws = ctx.workspace()?;
    let mut out = Vec::new();
    for (id, entry) in ws.closure() {
        match entry {
            Err(e) => out.push(DepStatus {
                name: id.to_string(),
                linked: false,
                root: None,
                version: None,
                last_indexed_commit: None,
                commits_behind_head: 0,
                nodes: 0,
                index: "missing".to_string(),
                embed_provider: None,
                note: Some(e.to_string()),
            }),
            Ok(handle) => {
                let db = Repo::index_db_at(&handle.root);
                let mut dep = DepStatus {
                    name: id.to_string(),
                    linked: true,
                    root: Some(handle.root.display().to_string()),
                    version: Some(handle.config.version.clone()),
                    last_indexed_commit: None,
                    commits_behind_head: 0,
                    nodes: 0,
                    index: "missing".to_string(),
                    embed_provider: None,
                    note: None,
                };
                if db.exists() {
                    match Index::open(&db) {
                        Ok(index) => {
                            dep.index = match index.schema_version() {
                                Some(v) if v == SCHEMA_VERSION => "fresh".to_string(),
                                _ => "stale-schema".to_string(),
                            };
                            dep.last_indexed_commit = index.meta("last_indexed_commit")?;
                            dep.embed_provider = index.meta("embed_provider")?;
                            dep.nodes =
                                index.scalar_i64("SELECT count(*) FROM nodes", ())? as usize;
                            if let Some(commit) = &dep.last_indexed_commit {
                                dep.commits_behind_head =
                                    crate::git::commits_ahead(&handle.root, commit).unwrap_or(0);
                            }
                        }
                        Err(_) => dep.index = "unreadable".to_string(),
                    }
                }
                out.push(dep);
            }
        }
    }
    Ok(out)
}
