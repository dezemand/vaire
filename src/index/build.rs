//! Building the index — `vaire index` (cli.md §4.1, design.md §9).
//!
//! Indexing is **bound to commit**: it reads the *committed* tree (commit-as-publish),
//! so every index state corresponds to exactly one commit. Two modes:
//!
//! - **incremental** (default): `git diff` the last-indexed commit → changed files →
//!   re-parse only those. The content-hash embedding cache retains unchanged section vectors,
//!   while all misses from the invocation share bounded provider batches.
//! - **`--full`**: drop and recreate `index.db`, re-parse everything.
//!
//! This is the command a `post-commit` git hook calls.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use crate::config::Config;
use crate::corpus::frontmatter;
use crate::corpus::repo::Repo;
use crate::corpus::scan::Scanner;
use crate::corpus::section::Section;
use crate::embed::{Embedder, cache};
use crate::error::Result;
use crate::index::db::{Index, col_blob, col_i64, col_text, col_u32};
use crate::model::node::Node;
use crate::model::reference::Reference;
use crate::search::vector::{decode_vector, encode_vector};

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

/// Parsed content ready for insertion. Parsing and embedding happen before the write
/// transaction so a remote embedding provider never holds the index lock.
struct PreparedNode {
    node: Node,
    prose_start: u32,
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

    let index = if incremental {
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

    // Classification (design.md §6): identification already happened syntactically in the
    // parser — whatever reached `node.edges` matched the strict target grammar. This step
    // consults the *local* vocabulary: a local frontmatter candidate becomes an edge only
    // when its type is declared (an undeclared one is dropped here and surfaced by
    // `vaire check` as unknown_type — never silenced). A cross-package `@pkg/` candidate is
    // classified by its *owning* package, not this one, so local vocabulary never gates it;
    // inline `[[...]]` are deliberate, so they're not gated either.
    let configured: std::collections::HashSet<&str> =
        config.types.iter().map(String::as_str).collect();

    // Fetch committed blobs in one Git session before the write transaction. Besides avoiding
    // one process per file, this keeps the transaction short rather than holding it across I/O.
    let mut committed_contents = committed
        .then(|| crate::git::show_many_at_head(root, &to_index))
        .transpose()?;

    // Gather every node before taking the write lock. This lets cache misses share embedding
    // batches across the corpus instead of limiting one provider request to one file.
    let mut prepared = Vec::new();
    for (position, rel) in to_index.iter().enumerate() {
        let content = if committed {
            committed_contents
                .as_mut()
                .expect("committed content fetched")[position]
                .take()
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
                        e.to.package().is_some() || configured.contains(e.to.node_type.as_str())
                    }
                });
                apply_scoping(&mut node, &config.scope_field);
                prepared.push(PreparedNode { node, prose_start });
            }
        }
    }
    let vectors = prepare_embeddings(&index, &prepared, embedder, incremental)?;

    index.with_tx(|index| {
        for rel in &to_delete {
            delete_file(index, rel)?;
        }
        for rel in &to_index {
            delete_file(index, rel)?; // idempotent: clear any prior rows for this path
        }
        for prepared in &prepared {
            index_node(
                index,
                &prepared.node,
                &config.name,
                prepared.prose_start,
                &vectors,
            )?;
        }
        // Now that every node is present, resolve relative scoped references scope-first.
        resolve_scoped_edges(index)?;
        Ok(())
    })?;

    // A recreated index intentionally deferred FTS construction until every section was
    // present. Incremental runs open an existing index, whose FTS structure already exists.
    if !incremental {
        index.ensure_fts_index()?;
    }

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
    // Which package this index belongs to (workspace sanity check, cli.md §6.5) and which
    // embedder produced its vectors (the content-hash cache keys on text only, so this
    // identity is the guard against mixing providers/models in one index).
    index.set_meta("package_name", &config.name)?;
    index.set_meta("embed_provider", &embedder.identity())?;

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

/// Resolve vectors for every section in one indexing invocation. On incremental updates,
/// cached vectors are retained; misses are de-duplicated by content hash and passed to the
/// provider in the same bounded batches used by `--re-embed`.
fn prepare_embeddings(
    index: &Index,
    prepared: &[PreparedNode],
    embedder: &dyn Embedder,
    reuse_cache: bool,
) -> Result<HashMap<cache::ContentHash, Vec<f32>>> {
    let mut vectors = HashMap::new();
    let mut misses: Vec<(cache::ContentHash, String)> = Vec::new();
    let mut seen = HashSet::new();

    for prepared in prepared {
        for section in Section::split(&prepared.node.prose, prepared.prose_start) {
            let hash = cache::hash_text(&section.body);
            if !seen.insert(hash) {
                continue;
            }
            if reuse_cache && let Some(vector) = cache_get(index, &hash)? {
                vectors.insert(hash, vector);
            } else {
                misses.push((hash, section.body));
            }
        }
    }

    for chunk in misses.chunks(REEMBED_BATCH) {
        let bodies: Vec<String> = chunk.iter().map(|(_, body)| body.clone()).collect();
        let embedded = embedder.embed(&bodies)?;
        if embedded.len() != chunk.len() {
            return Err(crate::error::VaireError::Config(format!(
                "embedder returned {} vectors for {} sections",
                embedded.len(),
                chunk.len()
            )));
        }
        for ((hash, _), vector) in chunk.iter().zip(embedded) {
            vectors.insert(*hash, vector);
        }
    }

    Ok(vectors)
}

/// Re-embed every section in the existing index with the current provider, bypassing the
/// content-hash cache (`vaire index --re-embed`, cli.md §4.1).
///
/// Use after changing the embedding model/provider/dimensions: the cache is keyed by
/// section text only, so a normal reindex would reuse the old model's vectors for
/// unchanged sections. This re-embeds from the already-indexed section bodies — no
/// re-parse, no Git read — leaving nodes/edges and the commit anchor untouched.
pub fn reembed(repo: &Repo, embedder: &dyn Embedder) -> Result<IndexSummary> {
    let started = Instant::now();
    let index = Index::open(&repo.index_db())?; // exit 4 if not built yet

    // Snapshot the sections to re-embed.
    let sections: Vec<(String, u32, String)> = index.query_rows(
        "SELECT node_id, line, body FROM sections ORDER BY node_id, line",
        (),
        |r| Ok((col_text(r, 0)?, col_u32(r, 1)?, col_text(r, 2)?)),
    )?;

    let embedded = index.with_tx(|index| {
        // Drop stale vectors and the cache so every section is re-embedded fresh.
        index.execute("DELETE FROM embeddings", ())?;
        index.execute("DELETE FROM embed_cache", ())?;

        let mut embedded = 0usize;
        for chunk in sections.chunks(REEMBED_BATCH) {
            let bodies: Vec<String> = chunk.iter().map(|(_, _, body)| body.clone()).collect();
            let vectors = embedder.embed(&bodies)?;
            for ((node_id, line, body), vector) in chunk.iter().zip(vectors) {
                let hash = cache::hash_text(body);
                index.execute(
                    "INSERT OR IGNORE INTO embed_cache(content_hash, vector) VALUES(?1, ?2)",
                    turso::params![hash.as_slice(), encode_vector(&vector)],
                )?;
                index.execute(
                    "INSERT INTO embeddings(node_id, section_line, content_hash, vector)
                     VALUES(?1, ?2, ?3, ?4)",
                    turso::params![
                        node_id.as_str(),
                        i64::from(*line),
                        hash.as_slice(),
                        encode_vector(&vector)
                    ],
                )?;
                embedded += 1;
            }
        }
        Ok(embedded)
    })?;

    // The vectors just changed hands — record the new provider identity.
    index.set_meta("embed_provider", &embedder.identity())?;

    Ok(IndexSummary {
        nodes: count(&index, "SELECT count(*) FROM nodes")?,
        edges: count(&index, "SELECT count(*) FROM edges")?,
        sections_embedded: embedded,
        elapsed_ms: started.elapsed().as_millis(),
        commit: index.meta("last_indexed_commit")?, // anchor unchanged
    })
}

/// Drop the index file (and its WAL sidecars) and recreate the schema. Also guarantees
/// the derived dir carries its self-contained `.gitignore` (design.md §9) — an index can
/// be created in a package that never ran `vaire init` (notably a linked dependency built
/// by a consumer's ensure pass), and derived files must never show up as untracked noise
/// in that package's repo.
fn recreate(db_path: &Path) -> Result<Index> {
    for suffix in ["", "-wal", "-shm"] {
        let p = format!("{}{suffix}", db_path.display());
        let _ = std::fs::remove_file(p);
    }
    if let Some(vaire_dir) = db_path.parent() {
        std::fs::create_dir_all(vaire_dir)?;
        Repo::ensure_derived_gitignore(vaire_dir)?;
    }
    Index::create_for_bulk_load(db_path)
}

/// All files tracked at HEAD that match the include/exclude globs.
fn committed_matching(root: &Path, scanner: &Scanner) -> Result<Vec<String>> {
    Ok(crate::git::list_files_at_head(root)?
        .into_iter()
        .filter(|rel| scanner.is_match(Path::new(rel)))
        .collect())
}

/// Apply scoping to a freshly-parsed node (cli.md §6.1) — purely from the node's own
/// frontmatter, no cross-file lookup. Scoping is **data-driven**: any node that carries the
/// configured `scope_field` (default `scope`) gets the composed address
/// `<container-id>/<type>:<local>`, regardless of type. Edge targets are left as parsed
/// (bare); relative scoped references are resolved **scope-first** after all nodes exist —
/// see [`resolve_scoped_edges`].
fn apply_scoping(node: &mut Node, scope_field: &str) {
    if let Some(container) = node.frontmatter.get(scope_field).and_then(|v| v.as_str()) {
        node.id.scope = Some(container.to_string());
    }
    let from = node.id.clone();
    for edge in &mut node.edges {
        edge.from = from.clone();
    }
}

/// Resolve relative scoped references, **scope-first then global**, once every node is in the
/// index (cli.md §6.1). For each edge from a scoped node to a *bare* target, if a node at
/// `<referrer-scope>/<target>` exists it is the sibling being referenced, so the edge is
/// rewritten to that composed id; otherwise the bare (global) target stands. Existence-based,
/// so it needs no type list and never scopes a reference that lacks a scoped sibling.
fn resolve_scoped_edges(index: &Index) -> Result<()> {
    let candidates: Vec<(i64, String, String)> = index.query_rows(
        // Local edges only: scope-first resolution finds a sibling in *this* package;
        // a cross-package target (to_package set) is resolved across the workspace in M5.
        "SELECT rowid, from_id, to_id FROM edges
         WHERE from_id LIKE '%/%' AND to_id NOT LIKE '%/%' AND to_package IS NULL",
        (),
        |r| Ok((col_i64(r, 0)?, col_text(r, 1)?, col_text(r, 2)?)),
    )?;

    let mut rewrites: Vec<(i64, String)> = Vec::new();
    for (rowid, from_id, to_id) in candidates {
        // The referrer's scope is everything before the last '/'.
        if let Some((scope, _)) = from_id.rsplit_once('/') {
            let candidate = format!("{scope}/{to_id}");
            let exists = index
                .query_opt(
                    "SELECT 1 FROM nodes WHERE id = ?1",
                    [candidate.as_str()],
                    |_| Ok(()),
                )?
                .is_some();
            if exists {
                rewrites.push((rowid, candidate));
            }
        }
    }
    for (rowid, to_id) in rewrites {
        index.execute(
            "UPDATE edges SET to_id = ?1 WHERE rowid = ?2",
            turso::params![to_id.as_str(), rowid],
        )?;
    }
    Ok(())
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

/// Insert one node and all its derived rows. `package` is the owning package (the manifest
/// `name`) — every node this build produces belongs to it (M4); cross-package *targets*
/// carry their own package on the edge.
fn index_node(
    index: &Index,
    node: &Node,
    package: &str,
    prose_start: u32,
    vectors: &HashMap<cache::ContentHash, Vec<f32>>,
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

    index.execute(
        "INSERT OR IGNORE INTO nodes(id, type, path, frontmatter, superseded_by, package)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        turso::params![
            id.as_str(),
            node.node_type().to_string(),
            node.path.as_str(),
            fm_json.as_str(),
            node.superseded_by().map(|s| s.to_string()),
            package,
        ],
    )?;
    index.execute(
        "INSERT INTO node_files(id, path) VALUES(?1, ?2)",
        turso::params![id.as_str(), node.path.as_str()],
    )?;

    for e in &node.edges {
        // The package travels in its own column; `to_id` is always the bare within-package
        // address, so a local edge and a cross-package edge to the same address share a
        // `to_id` and M5 resolution can match on it directly.
        index.execute(
            "INSERT INTO edges(from_id, to_id, to_package, ref_type, source_file, line)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            turso::params![
                id.as_str(),
                e.to.within_package(),
                e.to.package(),
                e.origin.as_ref_type(),
                e.source_file.as_str(),
                i64::from(e.line),
            ],
        )?;
    }

    for (reference, line) in &node.unresolved {
        if let Reference::Unresolved {
            type_guess,
            descriptor,
        } = reference
        {
            index.execute(
                "INSERT INTO unresolved(record_id, type_guess, descriptor, source_file, line)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                turso::params![
                    id.as_str(),
                    type_guess.as_ref().map(|t| t.to_string()),
                    descriptor.as_str(),
                    node.path.as_str(),
                    i64::from(*line),
                ],
            )?;
        }
    }

    // Sections → FTS + per-section embeddings (the file is the returned unit). Vectors
    // were prepared across the whole corpus before this transaction began.
    let sections = Section::split(&node.prose, prose_start);
    let hashes: Vec<[u8; 32]> = sections.iter().map(|s| cache::hash_text(&s.body)).collect();

    for (i, section) in sections.iter().enumerate() {
        let vector = vectors.get(&hashes[i]).ok_or_else(|| {
            crate::error::VaireError::Config(format!(
                "no prepared embedding vector for section at {}:{}",
                node.path, section.line
            ))
        })?;
        cache_put(index, &hashes[i], vector)?;
        index.execute(
            "INSERT INTO sections(node_id, heading, line, body) VALUES(?1, ?2, ?3, ?4)",
            turso::params![
                id.as_str(),
                section.heading.clone().unwrap_or_default(),
                i64::from(section.line),
                section.body.as_str(),
            ],
        )?;
        index.execute(
            "INSERT INTO embeddings(node_id, section_line, content_hash, vector)
             VALUES(?1, ?2, ?3, ?4)",
            turso::params![
                id.as_str(),
                i64::from(section.line),
                hashes[i].as_slice(),
                encode_vector(vector),
            ],
        )?;
    }

    Ok(())
}

/// Look up a cached embedding by content hash.
fn cache_get(index: &Index, hash: &[u8; 32]) -> Result<Option<Vec<f32>>> {
    let blob = index.query_opt(
        "SELECT vector FROM embed_cache WHERE content_hash = ?1",
        [hash.as_slice()],
        |r| col_blob(r, 0),
    )?;
    Ok(blob.map(|b| decode_vector(&b)))
}

/// Store an embedding in the cache (no-op on hash collision — same text, same vector).
fn cache_put(index: &Index, hash: &[u8; 32], vector: &[f32]) -> Result<()> {
    index.execute(
        "INSERT OR IGNORE INTO embed_cache(content_hash, vector) VALUES(?1, ?2)",
        turso::params![hash.as_slice(), encode_vector(vector)],
    )?;
    Ok(())
}

/// Remove every row derived from `rel` (idempotent).
fn delete_file(index: &Index, rel: &str) -> Result<()> {
    let ids: Vec<String> =
        index.query_rows("SELECT id FROM nodes WHERE path = ?1", [rel], |r| {
            col_text(r, 0)
        })?;
    for id in &ids {
        index.execute("DELETE FROM sections WHERE node_id = ?1", [id.as_str()])?;
        index.execute("DELETE FROM embeddings WHERE node_id = ?1", [id.as_str()])?;
    }
    index.execute("DELETE FROM nodes WHERE path = ?1", [rel])?;
    index.execute("DELETE FROM node_files WHERE path = ?1", [rel])?;
    index.execute("DELETE FROM edges WHERE source_file = ?1", [rel])?;
    index.execute("DELETE FROM unresolved WHERE source_file = ?1", [rel])?;
    Ok(())
}

fn count(index: &Index, sql: &str) -> Result<usize> {
    Ok(index.scalar_i64(sql, ())? as usize)
}
