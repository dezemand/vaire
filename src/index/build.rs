//! Building the index — `vaire index` (cli.md §4.1, design.md §9).
//!
//! Indexing is **bound to commit**: it reads the *committed* tree (commit-as-publish),
//! so every index state corresponds to exactly one commit. Two modes:
//!
//! - **incremental** (default): `git diff` the last-indexed commit → changed files →
//!   re-parse only those. (The content-hash embedding cache that makes this cheap when
//!   embeddings exist is step 4; for now changed sections are re-embedded directly.)
//! - **`--full`**: drop and recreate `index.db`, re-parse everything.
//!
//! This is the command a `post-commit` git hook calls.

use std::path::Path;
use std::time::Instant;

use rusqlite::{Transaction, params};

use crate::config::Config;
use crate::corpus::frontmatter;
use crate::corpus::repo::Repo;
use crate::corpus::scan::Scanner;
use crate::corpus::section::Section;
use crate::embed::{Embedder, cache};
use crate::error::Result;
use crate::index::db::Index;
use crate::model::node::Node;
use crate::model::reference::Reference;

/// Summary printed on completion (cli.md §4.1); also the `--json` object.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexSummary {
    pub nodes: usize,
    pub edges: usize,
    pub sections_embedded: usize,
    pub elapsed_ms: u128,
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Incremental,
    Full,
    /// Full pass over the **working tree** (uncommitted edits), regardless of Git state.
    /// Opt-in (cli.md §4.1 `--working-tree`); the recorded commit is `null` since the
    /// index no longer corresponds to a commit. Default remains commit-as-publish.
    WorkingTree,
}

/// Build or rebuild the index (cli.md §4.1).
///
/// Source + mode are chosen from the corpus's Git state:
/// - **Git repo with commits** → the *committed* tree (commit-as-publish), incremental
///   by `git diff` when a prior index exists, else a full committed pass.
/// - **Not a Git repo, or no commits yet** → a full pass over the **working tree** read
///   from disk (so a fresh or non-Git corpus still indexes).
/// - **`--full`** → always a full rebuild (`Mode::Full`).
///
/// "Git repo" means the corpus root itself has `.git`; a corpus nested in a larger repo
/// is treated as non-Git and read from disk.
pub fn run(
    repo: &Repo,
    config: &Config,
    embedder: &dyn Embedder,
    mode: Mode,
) -> Result<IndexSummary> {
    let started = Instant::now();
    let root = repo.root();
    let db_path = repo.index_db();
    let scanner = Scanner::from_config(config)?;

    // `--working-tree` forces the on-disk source regardless of Git state.
    let working_tree = matches!(mode, Mode::WorkingTree);

    // Committed-tree source only when the corpus root is a Git repo with a HEAD, and the
    // working tree was not explicitly requested.
    let head = if repo.is_git_root() {
        crate::git::head(root)?
    } else {
        None
    };
    let committed = head.is_some() && !working_tree;
    let force_full = matches!(mode, Mode::Full | Mode::WorkingTree);

    // Incremental is possible only when the existing index is itself a **committed**
    // snapshot with a recorded commit. Crucially, this means a plain `vaire index` after
    // `--working-tree` is NOT incremental — it recreates from the committed tree, so the
    // index always restores to the last commit (it never inherits working-tree rows).
    let (last_commit, prior_source, schema_ok) = if committed && !force_full && db_path.exists() {
        let existing = Index::open(&db_path)?;
        (
            existing.meta("last_indexed_commit")?,
            existing.meta("index_source")?,
            existing.schema_version() == Some(crate::index::db::SCHEMA_VERSION),
        )
    } else {
        (None, None, false)
    };
    // Incremental requires a matching schema; a stale-schema index is fully rebuilt (which
    // recreates the db with the current schema + version).
    let incremental =
        schema_ok && last_commit.is_some() && prior_source.as_deref() == Some("committed");

    let mut index = if incremental {
        Index::open(&db_path)?
    } else {
        recreate(&db_path)?
    };

    let (to_index, to_delete) = if incremental {
        partition_changed(root, &scanner, last_commit.as_deref().unwrap())?
    } else if committed {
        (committed_matching(root, &scanner)?, Vec::new())
    } else {
        (working_matching(repo, &scanner)?, Vec::new())
    };

    // The configured type vocabulary: a frontmatter `type:id` is only treated as an edge
    // when its type is one of these, so a colon in a non-reference value (a title, a note)
    // isn't mistaken for a reference (cli.md §6, issue: spurious dangling_ref). Inline
    // `[[...]]` are deliberate, so they're not gated.
    let configured: std::collections::HashSet<&str> =
        config.id_prefixes.iter().map(String::as_str).collect();

    let tx = index.conn_mut().transaction()?;
    for rel in &to_delete {
        delete_file(&tx, rel)?;
    }
    for rel in &to_index {
        delete_file(&tx, rel)?; // idempotent: clear any prior rows for this path
        let content = if committed {
            crate::git::show_at_head(root, rel)?
        } else {
            std::fs::read_to_string(root.join(rel)).ok()
        };
        if let Some(content) = content
            && let Some(doc) = frontmatter::split(&content)
        {
            let prose_start = doc.prose_start_line;
            if let Some(mut node) = frontmatter::to_node(rel, doc) {
                node.edges.retain(|e| match &e.origin {
                    crate::model::edge::RefOrigin::Inline => true,
                    crate::model::edge::RefOrigin::Frontmatter(_) => {
                        configured.contains(e.to.node_type.as_str())
                    }
                });
                apply_scoping(&mut node, &config.scoped_types, &config.scope_field);
                index_node(&tx, &node, prose_start, embedder)?;
            }
        }
    }
    tx.commit()?;

    // A working-tree index does not correspond to a commit, so record null. The
    // `index_source` marker makes "plain index restores the last commit" an explicit
    // invariant rather than an emergent one (see the incremental decision above).
    let recorded = if working_tree { None } else { head };
    if let Some(commit) = &recorded {
        index.set_meta("last_indexed_commit", commit)?;
    }
    index.set_meta(
        "index_source",
        if working_tree {
            "working-tree"
        } else {
            "committed"
        },
    )?;

    Ok(IndexSummary {
        nodes: count(&index, "SELECT count(*) FROM nodes")?,
        edges: count(&index, "SELECT count(*) FROM edges")?,
        sections_embedded: count(&index, "SELECT count(*) FROM embeddings")?,
        elapsed_ms: started.elapsed().as_millis(),
        commit: recorded,
    })
}

/// How many sections to embed per provider call during a re-embed.
const REEMBED_BATCH: usize = 128;

/// Re-embed every section in the existing index with the current provider, bypassing the
/// content-hash cache (`vaire index --re-embed`, cli.md §4.1).
///
/// Use after changing the embedding model/provider/dimensions: the cache is keyed by
/// section text only, so a normal reindex would reuse the old model's vectors for
/// unchanged sections. This re-embeds from the already-indexed section bodies — no
/// re-parse, no Git read — leaving nodes/edges and the commit anchor untouched.
pub fn reembed(repo: &Repo, embedder: &dyn Embedder) -> Result<IndexSummary> {
    let started = Instant::now();
    let mut index = Index::open(&repo.index_db())?; // exit 4 if not built yet

    // Snapshot the sections to re-embed (release the read borrow before the transaction).
    let sections: Vec<(String, u32, String)> = {
        let mut stmt = index
            .conn()
            .prepare("SELECT node_id, line, body FROM sections_fts ORDER BY node_id, line")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<_, _>>()?
    };

    let tx = index.conn_mut().transaction()?;
    // Drop stale vectors and the cache so every section is re-embedded fresh.
    tx.execute("DELETE FROM embeddings", [])?;
    tx.execute("DELETE FROM embed_cache", [])?;

    let mut embedded = 0usize;
    for chunk in sections.chunks(REEMBED_BATCH) {
        let bodies: Vec<String> = chunk.iter().map(|(_, _, body)| body.clone()).collect();
        let vectors = embedder.embed(&bodies)?;
        for ((node_id, line, body), vector) in chunk.iter().zip(vectors) {
            let hash = cache::hash_text(body);
            tx.execute(
                "INSERT OR IGNORE INTO embed_cache(content_hash, vector) VALUES(?1, ?2)",
                params![hash.as_slice(), encode_vector(&vector)],
            )?;
            tx.execute(
                "INSERT INTO embeddings(node_id, section_line, content_hash, vector)
                 VALUES(?1, ?2, ?3, ?4)",
                params![node_id, line, hash.as_slice(), encode_vector(&vector)],
            )?;
            embedded += 1;
        }
    }
    tx.commit()?;

    Ok(IndexSummary {
        nodes: count(&index, "SELECT count(*) FROM nodes")?,
        edges: count(&index, "SELECT count(*) FROM edges")?,
        sections_embedded: embedded,
        elapsed_ms: started.elapsed().as_millis(),
        commit: index.meta("last_indexed_commit")?, // anchor unchanged
    })
}

/// Drop the index file (and its WAL sidecars) and recreate the schema.
fn recreate(db_path: &Path) -> Result<Index> {
    for suffix in ["", "-wal", "-shm"] {
        let p = format!("{}{suffix}", db_path.display());
        let _ = std::fs::remove_file(p);
    }
    Index::create(db_path)
}

/// All files tracked at HEAD that match the include/exclude globs.
fn committed_matching(root: &Path, scanner: &Scanner) -> Result<Vec<String>> {
    Ok(crate::git::list_files_at_head(root)?
        .into_iter()
        .filter(|rel| scanner.is_match(Path::new(rel)))
        .collect())
}

/// Apply scoping to a freshly-parsed node (cli.md §6.1) — purely from the node's own
/// frontmatter, no cross-file lookup. For a node of a scoped type that carries the
/// configured `scope_field` (default `project`), the address becomes
/// `<container-id>/<type>:<local>`; its **relative** scoped references (a scoped-type
/// target with no scope of its own) inherit that same container as their scope. A no-op
/// when scoping is off or the node lacks the field.
fn apply_scoping(node: &mut Node, scoped_types: &[String], scope_field: &str) {
    let is_scoped_type = scoped_types.iter().any(|t| t == node.id.node_type.as_str());
    if is_scoped_type
        && let Some(container) = node.frontmatter.get(scope_field).and_then(|v| v.as_str())
    {
        node.id.scope = Some(container.to_string());
    }

    let from = node.id.clone();
    for edge in &mut node.edges {
        edge.from = from.clone();
        if let Some(scope) = &from.scope {
            // Relative scoped ref (scoped-type target with no scope) → this node's scope.
            if scoped_types.iter().any(|t| t == edge.to.node_type.as_str())
                && edge.to.scope.is_none()
            {
                edge.to.scope = Some(scope.clone());
            }
        }
    }
}

/// All matching files in the working tree on disk (the non-Git / fresh-repo path).
fn working_matching(repo: &Repo, scanner: &Scanner) -> Result<Vec<String>> {
    Ok(scanner
        .candidates(repo)?
        .into_iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect())
}

/// Split the files changed since `last` into (to-reindex, to-delete). A changed path
/// still present at HEAD is reindexed; one absent from HEAD is deleted.
fn partition_changed(
    root: &Path,
    scanner: &Scanner,
    last: &str,
) -> Result<(Vec<String>, Vec<String>)> {
    let head_set: std::collections::HashSet<String> =
        crate::git::list_files_at_head(root)?.into_iter().collect();
    let mut to_index = Vec::new();
    let mut to_delete = Vec::new();
    for rel in crate::git::changed_files(root, last)? {
        if !scanner.is_match(Path::new(&rel)) {
            continue;
        }
        if head_set.contains(&rel) {
            to_index.push(rel);
        } else {
            to_delete.push(rel);
        }
    }
    Ok((to_index, to_delete))
}

/// Insert one node and all its derived rows.
fn index_node(
    tx: &Transaction,
    node: &Node,
    prose_start: u32,
    embedder: &dyn Embedder,
) -> Result<()> {
    let id = node.id.to_string();

    // Frontmatter is stored as JSON (the YAML map serializes cleanly for our scalars).
    // The effective display name is resolved here (name → sole H1 → filename, design.md
    // §6) and stored under `name`, so resolve/render get a name even when the frontmatter
    // omits one.
    let mut fm_value =
        serde_json::to_value(&node.frontmatter).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = fm_value.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(node.display_name()),
        );
    }
    let fm_json = fm_value.to_string();

    tx.execute(
        "INSERT OR IGNORE INTO nodes(id, type, path, frontmatter, superseded_by)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![
            id,
            node.node_type().to_string(),
            node.path,
            fm_json,
            node.superseded_by().map(|s| s.to_string()),
        ],
    )?;
    tx.execute(
        "INSERT INTO node_files(id, path) VALUES(?1, ?2)",
        params![id, node.path],
    )?;

    for e in &node.edges {
        tx.execute(
            "INSERT INTO edges(from_id, to_id, ref_type, source_file, line)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                e.to.to_string(),
                e.origin.as_ref_type(),
                e.source_file,
                e.line
            ],
        )?;
    }

    for (reference, line) in &node.unresolved {
        if let Reference::Unresolved {
            type_guess,
            descriptor,
        } = reference
        {
            tx.execute(
                "INSERT INTO unresolved(record_id, type_guess, descriptor, source_file, line)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    type_guess.as_ref().map(|t| t.to_string()),
                    descriptor,
                    node.path,
                    line,
                ],
            )?;
        }
    }

    // Sections → FTS + per-section embeddings (the file is the returned unit). Each
    // section's vector comes from the content-hash cache when its text is unchanged;
    // only cache misses are embedded (design.md §9).
    let sections = Section::split(&node.prose, prose_start);
    let hashes: Vec<[u8; 32]> = sections.iter().map(|s| cache::hash_text(&s.body)).collect();

    // Resolve vectors: cache hit, or queue for a single batched embed call.
    let mut vectors: Vec<Option<Vec<f32>>> = Vec::with_capacity(sections.len());
    let mut misses: Vec<usize> = Vec::new();
    for (i, hash) in hashes.iter().enumerate() {
        match cache_get(tx, hash)? {
            Some(v) => vectors.push(Some(v)),
            None => {
                vectors.push(None);
                misses.push(i);
            }
        }
    }
    if !misses.is_empty() {
        let bodies: Vec<String> = misses.iter().map(|&i| sections[i].body.clone()).collect();
        let embedded = embedder.embed(&bodies)?;
        for (&i, vector) in misses.iter().zip(embedded) {
            cache_put(tx, &hashes[i], &vector)?;
            vectors[i] = Some(vector);
        }
    }

    for (i, section) in sections.iter().enumerate() {
        let vector = vectors[i].as_ref().expect("every section has a vector");
        tx.execute(
            "INSERT INTO sections_fts(node_id, heading, line, body) VALUES(?1, ?2, ?3, ?4)",
            params![
                id,
                section.heading.clone().unwrap_or_default(),
                section.line,
                section.body
            ],
        )?;
        tx.execute(
            "INSERT INTO embeddings(node_id, section_line, content_hash, vector)
             VALUES(?1, ?2, ?3, ?4)",
            params![
                id,
                section.line,
                hashes[i].as_slice(),
                encode_vector(vector)
            ],
        )?;
    }

    Ok(())
}

/// Look up a cached embedding by content hash.
fn cache_get(tx: &Transaction, hash: &[u8; 32]) -> Result<Option<Vec<f32>>> {
    use rusqlite::OptionalExtension;
    let blob: Option<Vec<u8>> = tx
        .query_row(
            "SELECT vector FROM embed_cache WHERE content_hash = ?1",
            [hash.as_slice()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(blob.map(|b| crate::search::vector::decode_vector(&b)))
}

/// Store an embedding in the cache (no-op on hash collision — same text, same vector).
fn cache_put(tx: &Transaction, hash: &[u8; 32], vector: &[f32]) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO embed_cache(content_hash, vector) VALUES(?1, ?2)",
        params![hash.as_slice(), encode_vector(vector)],
    )?;
    Ok(())
}

/// Remove every row derived from `rel` (idempotent).
fn delete_file(tx: &Transaction, rel: &str) -> Result<()> {
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM nodes WHERE path = ?1")?;
        let rows = stmt.query_map([rel], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for id in &ids {
        tx.execute("DELETE FROM sections_fts WHERE node_id = ?1", [id])?;
        tx.execute("DELETE FROM embeddings WHERE node_id = ?1", [id])?;
    }
    tx.execute("DELETE FROM nodes WHERE path = ?1", [rel])?;
    tx.execute("DELETE FROM node_files WHERE path = ?1", [rel])?;
    tx.execute("DELETE FROM edges WHERE source_file = ?1", [rel])?;
    tx.execute("DELETE FROM unresolved WHERE source_file = ?1", [rel])?;
    Ok(())
}

/// Encode a vector as little-endian f32 bytes (brute-force cosine decodes it — §search).
fn encode_vector(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

fn count(index: &Index, sql: &str) -> Result<usize> {
    let n: i64 = index.conn().query_row(sql, [], |r| r.get(0))?;
    Ok(n as usize)
}
