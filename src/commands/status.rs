//! `vaire status` (cli.md §4.3). The one read-adjacent command that tolerates a
//! missing index: it reports `last_indexed_commit: null` and exits `0`.

use std::collections::BTreeMap;

use crate::commands::Ctx;
use crate::error::Result;
use crate::index::db::{Index, col_i64, col_text};
use crate::output::{EmbeddingCounts, NodeCounts, StatusOutput};

pub fn run(ctx: &Ctx) -> Result<StatusOutput> {
    let repo = &ctx.repo;
    let repo_path = repo.root().display().to_string();
    let index_path = ".vaire/index.db".to_string();

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
        });
    }

    let index = Index::open(&repo.index_db())?;
    let schema_version = index.schema_version();
    let last_indexed_commit = index.meta("last_indexed_commit")?;
    let source = index.meta("index_source")?;

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
    })
}
