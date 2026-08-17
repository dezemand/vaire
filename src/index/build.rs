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
use crate::index::db::{Index, col_blob, col_text, col_u32};
use crate::model::edge::Edge;
use crate::model::id::NodeId;
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
    let db_path = Repo::prepare_derived_dir(root)?.join("index.db");
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
    let (last_commit, prior_source, schema_ok, provider_ok) =
        if committed && !force_full && db_path.exists() {
            let existing = Index::open(&db_path)?;
            let last_commit = existing
                .meta("last_indexed_commit")?
                .filter(|commit| crate::git::is_commit_oid(commit));
            // A provider switch must not mix vector spaces. `dep_mode` enforced this for
            // *dependencies* only, while the current package always went incremental — so
            // changed sections were embedded by the new model, unchanged ones kept the old
            // model's vectors (the cache keys on text alone), and the `embed_provider` meta
            // was then re-stamped below with the new identity. That certified a mixed index
            // as homogeneous and destroyed the only evidence that `--re-embed` was still
            // owed. Equal-width spaces are the dangerous case: the `length(vector)` filter
            // in search cannot see them, so ranking silently degrades.
            let provider_ok = existing
                .meta("embed_provider")?
                .is_none_or(|prior| prior == embedder.identity());
            (
                last_commit,
                existing.meta("index_source")?,
                existing.schema_version() == Some(crate::index::db::SCHEMA_VERSION),
                provider_ok,
            )
        } else {
            (None, None, false, false)
        };
    // Incremental requires a matching schema; a stale-schema index is fully rebuilt (which
    // recreates the db with the current schema + version).
    let incremental = schema_ok
        && provider_ok
        && last_commit.is_some()
        && prior_source.as_deref() == Some("committed");

    // Resolve the changeset *before* touching the index file: if Git cannot diff from the
    // recorded anchor (history rewritten and gc'd, shallow clone), there is no honest
    // incremental answer, so the run downgrades to a full rebuild. Reporting success with
    // an empty diff would strand every intervening edit — the anchor advances to HEAD
    // below, so nothing would ever re-examine those commits.
    let incremental_changes = match incremental {
        true => partition_changed(root, &scanner, last_commit.as_deref().unwrap(), &db_path)?,
        false => None,
    };
    let incremental = incremental_changes.is_some();

    // A full rebuild is staged beside the real index and promoted by an atomic rename only
    // once it is complete. Recreating in place meant any failure after the unlink — a 5xx
    // from the embedding provider, a stalled request, ctrl-C — left a schema-valid but
    // EMPTY index that `open_index` happily accepts, so `resolve` reported not-found and
    // MCP `search` returned [] with isError:false, silently, until someone noticed.
    let staged = (!incremental).then(|| staging_path(&db_path));
    let index = match &staged {
        None => Index::open(&db_path)?,
        Some(path) => recreate(root, path)?,
    };

    // The content-hash embedding cache lives inside the index file, so a full rebuild used
    // to discard it along with the old db — making every rebuild of a non-Git corpus (and
    // every `--working-tree` run, the edit-iterate loop) re-embed the whole corpus, against
    // design.md §9's promise that only changed sections are re-embedded. Keep the previous
    // index open purely as a cache source; it is never written and is dropped before the
    // promote below.
    //
    // The cache is keyed on section text alone, so it may only be carried across when the
    // *same* embedder produced it — otherwise a rebuild triggered by a provider switch
    // would hand the old model's vectors straight back.
    let previous = match &staged {
        None => None,
        Some(_) if !db_path.exists() => None,
        Some(_) => Index::open(&db_path).ok().filter(|old| {
            old.schema_version() == Some(crate::index::db::SCHEMA_VERSION)
                && old
                    .meta("embed_provider")
                    .ok()
                    .flatten()
                    .is_none_or(|prior| prior == embedder.identity())
        }),
    };

    let (to_index, to_delete) = match incremental_changes {
        Some(changes) => changes,
        None if committed => (committed_matching(root, &scanner)?, Vec::new()),
        None => (working_matching(repo, &scanner)?, Vec::new()),
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
            && let Some(node) = prepare_node(rel, &content, config, &configured)
        {
            prepared.push(node);
        }
    }
    // External diagram files (issue #23): a `.puml`/`.mmd`/etc. a node's prose links to
    // is not a corpus node — it's not in `to_index`, matches no include glob — so its
    // `vaire/` markers have to be fetched by path once the referencing set is known.
    // Diagram files are frequently shared by several nodes, so fetch each path once.
    let mut diagram_paths: Vec<String> = Vec::new();
    for prepared in &prepared {
        for path in diagram_link_paths(&prepared.node.path, &prepared.node.prose) {
            if !diagram_paths.contains(&path) {
                diagram_paths.push(path);
            }
        }
    }
    if !diagram_paths.is_empty() {
        let diagram_contents: HashMap<String, String> = if committed {
            crate::git::show_many_at_head(root, &diagram_paths)?
                .into_iter()
                .zip(diagram_paths.iter())
                .filter_map(|(content, path)| Some((path.clone(), content?)))
                .collect()
        } else {
            diagram_paths
                .iter()
                .filter_map(|path| {
                    std::fs::read_to_string(root.join(path))
                        .ok()
                        .map(|c| (path.clone(), c))
                })
                .collect()
        };
        for prepared in &mut prepared {
            for path in diagram_link_paths(&prepared.node.path, &prepared.node.prose) {
                if let Some(content) = diagram_contents.get(&path) {
                    prepared.node.edges.extend(external_diagram_edges(
                        &prepared.node.id,
                        &path,
                        content,
                    ));
                }
            }
        }
    }
    // The same target can be marked in more than one fenced block or linked diagram
    // file for one node — collapse to one edge per (from, to) regardless of source.
    for prepared in &mut prepared {
        crate::corpus::diagram::dedupe_diagram_edges(&mut prepared.node.edges);
    }

    // Read cached vectors from the live index when updating in place, or from the previous
    // index when staging a rebuild. Either way an unchanged section keeps its vector.
    let cache_source = previous.as_ref().unwrap_or(&index);
    let vectors = prepare_embeddings(cache_source, &prepared, embedder)?;

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
                Some(&vectors),
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

    let summary = IndexSummary {
        nodes: count(&index, "SELECT count(*) FROM nodes")?,
        edges: count(&index, "SELECT count(*) FROM edges")?,
        sections_embedded: count(&index, "SELECT count(*) FROM embeddings")?,
        elapsed_ms: started.elapsed().as_millis(),
        commit: recorded,
    };

    // Everything succeeded — swap the finished index in. Both handles must be closed
    // first so no connection still holds the files being renamed. Turso does NOT
    // checkpoint the WAL on close, which is exactly why `promote` moves the sidecars
    // along with the database instead of assuming the file is self-contained.
    if let Some(staged) = staged {
        drop(previous);
        drop(index);
        promote(&staged, &db_path)?;
    }

    Ok(summary)
}

/// Where a full rebuild is assembled before it replaces the live index.
fn staging_path(db_path: &Path) -> std::path::PathBuf {
    db_path.with_file_name("index.db.building")
}

/// Delete a database file and its WAL sidecars.
///
/// A missing file is fine; any other failure is reported. Ignoring errors here let a
/// rebuild continue against the *old* file when the unlink failed (e.g. a root-owned
/// `.vaire/`), where `CREATE ... IF NOT EXISTS` no-ops and the schema version is re-stamped
/// — quietly keeping ghost rows and branding an old-shaped db with the current version.
pub(crate) fn remove_db_files(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let sidecar = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Replace the live index with a completed staged build.
///
/// The WAL sidecar moves with the database: if a checkpoint has not folded it back into
/// the main file, renaming that file alone would leave the committed rows behind — the
/// promoted index would open with no schema version at all. WAL contents are positional,
/// not path-bound, so moving the set together lets the next open recover normally.
fn promote(staged: &Path, db_path: &Path) -> Result<()> {
    remove_db_files(db_path)?;
    std::fs::rename(staged, db_path)?;
    for suffix in ["-wal", "-shm"] {
        let from = format!("{}{suffix}", staged.display());
        let to = format!("{}{suffix}", db_path.display());
        match std::fs::rename(&from, &to) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// How many sections to embed per provider call during a re-embed.
const REEMBED_BATCH: usize = 128;

/// Resolve vectors for every section in one indexing invocation. On incremental updates,
/// cached vectors are retained; misses are de-duplicated by content hash and passed to the
/// provider in the same bounded batches used by `--re-embed`.
fn prepare_embeddings(
    cache_source: &Index,
    prepared: &[PreparedNode],
    embedder: &dyn Embedder,
) -> Result<HashMap<cache::ContentHash, Vec<f32>>> {
    let index = cache_source;
    let mut vectors = HashMap::new();
    let mut misses: Vec<(cache::ContentHash, String)> = Vec::new();
    let mut seen = HashSet::new();

    for prepared in prepared {
        for section in Section::split(&prepared.node.prose, prepared.prose_start) {
            let hash = cache::hash_text(&section.body);
            if !seen.insert(hash) {
                continue;
            }
            if let Some(vector) = cache_get(index, &hash)? {
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
    let db_path = Repo::prepare_derived_dir(repo.root())?.join("index.db");
    let index = Index::open(&db_path)?; // exit 4 if not built yet

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

/// Parse one file into an indexable node, applying the classification and scoping rules
/// (design.md §6, cli.md §6.1) every build path must apply identically.
///
/// Shared by the live build and [`snapshot`] on purpose: the release classifier compares
/// a snapshot against the live index, so any divergence between the two parse paths would
/// show up as a phantom entity change and mis-classify a release.
fn prepare_node(
    rel: &str,
    content: &str,
    config: &Config,
    configured: &HashSet<&str>,
) -> Option<PreparedNode> {
    let doc = frontmatter::split(content)?;
    let prose_start = doc.prose_start_line;
    let mut node = frontmatter::to_node(rel, doc)?;
    node.edges.retain(|e| match &e.origin {
        crate::model::edge::RefOrigin::Inline | crate::model::edge::RefOrigin::Diagram => true,
        crate::model::edge::RefOrigin::Frontmatter(_) => {
            e.to.package().is_some() || configured.contains(e.to.node_type.as_str())
        }
    });
    apply_scoping(&mut node, &config.scope_field);
    Some(PreparedNode { node, prose_start })
}

/// Relative diagram-file paths a node's prose links to (Markdown links/images whose
/// target names a diagram extension — issue #23), resolved against the node's own file
/// and de-duplicated. Reuses the same link-extraction the packer already applies to
/// prose, minus the `pack` feature gate.
fn diagram_link_paths(rel: &str, prose: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (target, _line) in crate::corpus::markdown::relative_link_targets(prose) {
        if !crate::corpus::diagram::is_diagram_path(&target) {
            continue;
        }
        if let Some(resolved) = crate::corpus::markdown::resolve_relative(rel, &target)
            && !out.contains(&resolved)
        {
            out.push(resolved);
        }
    }
    out
}

/// Scan an external diagram file's already-fetched content for `vaire/` markers and
/// turn resolved ones into edges from `node_id`. Per the issue, `source_file`/`line`
/// point at the diagram file itself — a reference lives where it is written.
fn external_diagram_edges(node_id: &NodeId, diagram_path: &str, content: &str) -> Vec<Edge> {
    crate::corpus::diagram::scan_source(content)
        .into_iter()
        .filter_map(|(marker, line)| match marker {
            crate::corpus::diagram::DiagramMarker::Resolved(target) => Some(Edge {
                from: node_id.clone(),
                to: target,
                origin: crate::model::edge::RefOrigin::Diagram,
                source_file: diagram_path.to_string(),
                line,
            }),
            crate::corpus::diagram::DiagramMarker::Malformed(_) => None,
        })
        .collect()
}

/// Build an index of the corpus **as it stood at `rev`**, into `db_path`.
///
/// The release classifier's baseline (registry.md §3.1): rather than keeping the last
/// release's artifact around to read its index out of, the tree at that release's tag is
/// re-indexed with today's code. `vaire pack` is byte-deterministic from a commit, so
/// this reproduces what that release shipped without any artifact archaeology — and it
/// keeps working for a package whose artifacts were never retained.
///
/// Three deliberate differences from a live build, all because the result is read once
/// and deleted:
///
/// * **No embeddings.** Vectors cost money and API calls and answer no question a diff
///   asks, so nothing here needs an embedding provider configured — which is what lets
///   `release` run in a bare CI container.
/// * **No FTS structure.** The diff reads `nodes`/`sections`/`edges` directly.
/// * **No promotion and no derived dir.** `db_path` is a scratch file the caller owns;
///   the package's own `.vaire/` is never touched, so a snapshot cannot disturb the live
///   index's incremental eligibility.
///
/// `config` is the manifest to parse *with* — the caller passes the one committed at
/// `rev`, since a package's own include globs and vocabulary at that release are what
/// decided which files were nodes.
pub fn snapshot(root: &Path, config: &Config, rev: &str, db_path: &Path) -> Result<()> {
    let scanner = Scanner::from_config(config)?;
    let files: Vec<String> = crate::git::list_files_at(root, rev)?
        .into_iter()
        .filter(|rel| scanner.is_match(Path::new(rel)))
        .collect();
    let contents = crate::git::show_many_at(root, rev, &files)?;
    let configured: HashSet<&str> = config.types.iter().map(String::as_str).collect();

    let mut prepared: Vec<PreparedNode> = files
        .iter()
        .zip(contents)
        .filter_map(|(rel, content)| prepare_node(rel, &content?, config, &configured))
        .collect();

    // External diagram files, same as the live build: fetched at the same `rev` so the
    // snapshot's edges are byte-identical to what a working-tree build at that commit
    // would produce.
    let mut diagram_paths: Vec<String> = Vec::new();
    for prepared in &prepared {
        for path in diagram_link_paths(&prepared.node.path, &prepared.node.prose) {
            if !diagram_paths.contains(&path) {
                diagram_paths.push(path);
            }
        }
    }
    if !diagram_paths.is_empty() {
        let diagram_contents: HashMap<String, String> =
            crate::git::show_many_at(root, rev, &diagram_paths)?
                .into_iter()
                .zip(diagram_paths.iter())
                .filter_map(|(content, path)| Some((path.clone(), content?)))
                .collect();
        for prepared in &mut prepared {
            for path in diagram_link_paths(&prepared.node.path, &prepared.node.prose) {
                if let Some(content) = diagram_contents.get(&path) {
                    prepared.node.edges.extend(external_diagram_edges(
                        &prepared.node.id,
                        &path,
                        content,
                    ));
                }
            }
        }
    }
    for prepared in &mut prepared {
        crate::corpus::diagram::dedupe_diagram_edges(&mut prepared.node.edges);
    }

    remove_db_files(db_path)?;
    let index = Index::create_for_bulk_load(db_path)?;
    index.with_tx(|index| {
        for prepared in &prepared {
            index_node(
                index,
                &prepared.node,
                &config.name,
                prepared.prose_start,
                None,
            )?;
        }
        resolve_scoped_edges(index)?;
        Ok(())
    })?;
    index.set_meta("package_name", &config.name)?;
    Ok(())
}

/// Drop the index file (and its WAL sidecars) and recreate the schema. Also guarantees
/// the derived dir carries its self-contained `.gitignore` (design.md §9) — an index can
/// be created in a package that never ran `vaire init` (notably a linked dependency built
/// by a consumer's ensure pass), and derived files must never show up as untracked noise
/// in that package's repo.
fn recreate(root: &Path, db_path: &Path) -> Result<Index> {
    let vaire_dir = Repo::prepare_derived_dir(root)?;
    remove_db_files(db_path)?;
    Repo::ensure_derived_gitignore(&vaire_dir)?;
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
        // Same forgiving strip the edge collector applies, so `project: "[[project:atlas]]"`
        // scopes to `project:atlas` instead of baking the brackets into the node's own id —
        // an id no reference could address, and one that disagreed with the edge derived
        // from that very field.
        node.id.scope = Some(frontmatter::strip_wikilink_brackets(container).to_string());
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
    // One set-based statement. This used to full-scan `edges` (the leading-wildcard LIKE
    // cannot use an index), then fire a separate `SELECT 1 FROM nodes WHERE id = ?` for
    // every candidate row and a separate UPDATE for every rewrite — each its own trip
    // through the block_on facade. Targets that never have a scoped sibling were re-probed
    // one query at a time on every build, forever.
    //
    // `rtrim(from_id, replace(from_id, '/', ''))` is the SQL spelling of "everything up to
    // and including the last '/'": the second argument is the set of characters to strip,
    // i.e. every character of from_id except '/', so trailing non-'/' characters come off
    // and the scope prefix (with its separator) remains.
    index.execute(
        "UPDATE edges
            SET to_id = rtrim(from_id, replace(from_id, '/', '')) || to_id
          WHERE from_id LIKE '%/%'
            AND to_id NOT LIKE '%/%'
            AND to_package IS NULL
            AND EXISTS (
                SELECT 1 FROM nodes n
                 WHERE n.id = rtrim(from_id, replace(from_id, '/', '')) || to_id
            )",
        (),
    )?;
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
/// `None` when Git cannot diff from `last` (unreachable after a rewrite, shallow clone).
/// The caller must rebuild fully rather than treat it as an empty changeset.
fn partition_changed(
    root: &Path,
    scanner: &Scanner,
    last: &str,
    db_path: &Path,
) -> Result<Option<(Vec<String>, Vec<String>)>> {
    let Some(changed) = crate::git::changed_files(root, last)? else {
        return Ok(None);
    };
    let head_set: std::collections::HashSet<String> =
        crate::git::list_files_at_head(root)?.into_iter().collect();
    let mut to_index = Vec::new();
    let mut to_delete = Vec::new();
    let mut diagram_changed = Vec::new();
    for rel in changed {
        if !scanner.is_match(Path::new(&rel)) {
            if crate::corpus::diagram::is_diagram_path(&rel) {
                diagram_changed.push(rel);
            }
            continue;
        }
        if head_set.contains(&rel) {
            to_index.push(rel);
        } else {
            to_delete.push(rel);
        }
    }
    // An edited external diagram file (`.puml`/`.mmd`/etc.) is not itself a corpus node,
    // so it never appears in `to_index` above and its edges would otherwise go stale
    // (issue #23). Force any node that *links* to a changed diagram file back into
    // `to_index`, so its content is re-fetched and re-scanned. This reads `diagram_links`
    // — the link relationship, recorded independent of whether a marker (and so an edge)
    // exists yet — rather than the `edges` table, so it also catches the first marker
    // ever added to a previously-marker-free diagram file, and a brand-new diagram file a
    // node already linked to (a dangling link still records a `diagram_links` row).
    if !diagram_changed.is_empty() && db_path.exists() {
        let existing = Index::open(db_path)?;
        for path in &diagram_changed {
            let referencing: Vec<String> = existing.query_rows(
                "SELECT DISTINCT n.path FROM diagram_links dl JOIN nodes n ON n.id = dl.from_id \
                 WHERE dl.path = ?1",
                [path.as_str()],
                |r| col_text(r, 0),
            )?;
            for rel in referencing {
                if head_set.contains(&rel) && !to_index.contains(&rel) {
                    to_index.push(rel);
                }
            }
        }
    }
    Ok(Some((to_index, to_delete)))
}

/// Insert one node and all its derived rows. `package` is the owning package (the manifest
/// `name`) — every node this build produces belongs to it (M4); cross-package *targets*
/// carry their own package on the edge.
/// `vectors` is `None` for a build that carries no embeddings at all ([`snapshot`]) —
/// sections are still stored (the diff and the FTS index are built over them), only the
/// vector rows are skipped. Within an embedding build a *missing* vector stays a hard
/// error: it would mean the prepare pass and this loop disagreed about the section set.
fn index_node(
    index: &Index,
    node: &Node,
    package: &str,
    prose_start: u32,
    vectors: Option<&HashMap<cache::ContentHash, Vec<f32>>>,
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

    // Denormalized match text for `alias_pass`: display name + aliases, lowercased once
    // here instead of re-derived (and re-JSON-parsed) for every node on every query.
    // Separated by U+001F (unit separator), never NUL: SQLite's LIKE treats an embedded
    // NUL as end-of-string, so aliases after the first would be invisible to the
    // narrowing filter in `alias_pass` and silently stop matching.
    let mut alias_text = node.display_name().to_lowercase();
    for alias in node.aliases() {
        alias_text.push(crate::search::ALIAS_SEP);
        alias_text.push_str(&alias.to_lowercase());
    }

    index.execute(
        "INSERT OR IGNORE INTO nodes(id, type, path, frontmatter, superseded_by, package, alias_text)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        turso::params![
            id.as_str(),
            node.node_type().to_string(),
            node.path.as_str(),
            fm_json.as_str(),
            node.superseded_by().map(|s| s.to_string()),
            package,
            alias_text.as_str(),
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

    // Every diagram file this node's prose links to — recorded independent of whether it
    // currently carries a `vaire/` marker (or exists at all), so `partition_changed` can
    // invalidate this node the moment such a file changes, not only once it already has
    // an edge to lose (issue #23).
    for path in diagram_link_paths(&node.path, &node.prose) {
        index.execute(
            "INSERT INTO diagram_links(from_id, path) VALUES(?1, ?2)",
            turso::params![id.as_str(), path.as_str()],
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
        index.execute(
            "INSERT INTO sections(node_id, heading, line, body) VALUES(?1, ?2, ?3, ?4)",
            turso::params![
                id.as_str(),
                section.heading.clone().unwrap_or_default(),
                i64::from(section.line),
                section.body.as_str(),
            ],
        )?;
        let Some(vectors) = vectors else {
            continue;
        };
        let vector = vectors.get(&hashes[i]).ok_or_else(|| {
            crate::error::VaireError::Config(format!(
                "no prepared embedding vector for section at {}:{}",
                node.path, section.line
            ))
        })?;
        cache_put(index, &hashes[i], vector)?;
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
    // A diagram edge's source_file names the diagram file, not this node's own file (the
    // marker lives where it's written) — so re-deriving this node's edges also means
    // clearing any diagram edges it previously originated, keyed by from_id instead.
    for id in &ids {
        index.execute(
            "DELETE FROM edges WHERE from_id = ?1 AND ref_type = 'diagram'",
            [id.as_str()],
        )?;
        index.execute(
            "DELETE FROM diagram_links WHERE from_id = ?1",
            [id.as_str()],
        )?;
    }
    index.execute("DELETE FROM unresolved WHERE source_file = ?1", [rel])?;
    Ok(())
}

fn count(index: &Index, sql: &str) -> Result<usize> {
    Ok(index.scalar_i64(sql, ())? as usize)
}
