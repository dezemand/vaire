//! Hybrid retrieval — FTS first, vectors for recall (design.md §9, cli.md §3.4).
//!
//! Three retrieval jobs want different things; this module serves open-ended `search`:
//! FTS5 + `aliases:` carry precision, vectors are the recall layer behind them. The
//! **file is the returned unit**, with the matching section anchors. Results sort by
//! descending score; ties break by `id` ascending for determinism.
//!
//! (Reference resolution, design.md §8, is the *same* machinery used in the other
//! direction — alias + FTS first, embeddings as backup — and will live alongside this.)

pub mod vector;

use crate::embed::Embedder;
use crate::error::Result;
use crate::index::db::{Index, col_f64, col_text, col_u32};
use crate::model::id::{NodeId, NodeType};

/// One search hit: a file plus the section anchors that matched (cli.md §3.4).
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: NodeId,
    pub node_type: NodeType,
    pub path: String,
    /// Opaque relative rank, not a calibrated probability.
    pub score: f32,
    pub anchors: Vec<Anchor>,
}

#[derive(Debug, Clone)]
pub struct Anchor {
    pub heading: String,
    pub line: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOpts {
    pub type_filter: Option<NodeType>,
    pub scope: Option<NodeId>,
    pub limit: Option<usize>,
    /// The frontmatter field that defines scope membership (config `scope_field`, e.g.
    /// `scope` or `project`) — the `ref_type` of the scope edge `--scope` filters on.
    pub scope_field: String,
}

// Scoring weights (opaque relative ranks, design.md §9 / cli.md §3.4). Alias/name hits
// are the highest-precision signal; FTS term frequency next; vectors are recall backup.
const ALIAS_WEIGHT: f32 = 5.0;
const FTS_WEIGHT: f32 = 1.0;
const VECTOR_WEIGHT: f32 = 1.0;
/// Vector recall only fires above this cosine. Deliberately high: the built-in
/// feature-hash embedder is not semantically meaningful, so vector-only matches stay
/// conservative until a real local model is plugged in (design.md §9). FTS + aliases
/// carry precision regardless.
const VECTOR_THRESHOLD: f32 = 0.9;
/// Cap anchors reported per file, so output stays readable.
const MAX_ANCHORS: usize = 3;

/// One node accumulating its score and matched section anchors across the passes.
struct Acc {
    node_type: String,
    path: String,
    score: f32,
    anchors: std::collections::BTreeMap<u32, Anchor>,
}

/// Run hybrid search. FTS + alias matches are scored first (precision); the vector pass
/// (brute-force cosine over the embeddings blob) adds recall; the passes merge per file
/// and rank by descending score, ties broken by `id` ascending (cli.md §3.4).
pub fn search(
    index: &Index,
    embedder: &dyn Embedder,
    query: &str,
    opts: &SearchOpts,
) -> Result<Vec<SearchHit>> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut acc: std::collections::BTreeMap<String, Acc> = std::collections::BTreeMap::new();

    fts_pass(index, &tokens, &mut acc)?;
    alias_pass(index, &tokens, &mut acc)?;
    vector_pass(index, embedder, query, &tokens, &mut acc)?;

    // Filters.
    if let Some(t) = &opts.type_filter {
        acc.retain(|_, a| a.node_type == t.as_str());
    }
    if let Some(scope) = &opts.scope {
        let in_scope = scope_set(index, scope, &opts.scope_field)?;
        acc.retain(|id, _| in_scope.contains(id));
    }

    // Assemble + rank.
    let mut hits: Vec<SearchHit> = acc
        .into_iter()
        .map(|(id, a)| {
            let mut anchors: Vec<Anchor> = a.anchors.into_values().collect();
            anchors.truncate(MAX_ANCHORS);
            SearchHit {
                id: id.parse().expect("stored id is well-formed"),
                node_type: NodeType::new(a.node_type),
                path: a.path,
                score: a.score,
                anchors,
            }
        })
        .collect();
    hits.sort_by(|x, y| {
        y.score
            .partial_cmp(&x.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.id.cmp(&y.id))
    });
    hits.truncate(opts.limit.unwrap_or(10));
    Ok(hits)
}

/// Native FTS over section headings + bodies. Candidate sections are found via `fts_match`
/// (Turso's Tantivy index; space-separated tokens are OR-combined), then scored by
/// query-term frequency in Rust — kept from the FTS5 era so ranking is transparent and
/// unchanged across the engine swap.
fn fts_pass(
    index: &Index,
    tokens: &[String],
    acc: &mut std::collections::BTreeMap<String, Acc>,
) -> Result<()> {
    let match_query = tokens.join(" ");
    let rows = index.query_rows(
        "SELECT s.node_id, s.heading, s.line, s.body, n.type, n.path
         FROM sections s JOIN nodes n ON n.id = s.node_id
         WHERE fts_match(s.heading, s.body, ?1)",
        [match_query.as_str()],
        |r| {
            Ok((
                col_text(r, 0)?,
                col_text(r, 1)?,
                col_u32(r, 2)?,
                col_text(r, 3)?,
                col_text(r, 4)?,
                col_text(r, 5)?,
            ))
        },
    )?;
    for (id, heading, line, body, node_type, path) in rows {
        let tf = term_frequency(&body, tokens);
        if tf == 0 {
            continue;
        }
        let entry = acc.entry(id).or_insert_with(|| Acc {
            node_type,
            path,
            score: 0.0,
            anchors: Default::default(),
        });
        entry.score += FTS_WEIGHT * tf as f32;
        entry.anchors.entry(line).or_insert_with(|| Anchor {
            heading,
            line,
            snippet: snippet(&body, tokens),
        });
    }
    Ok(())
}

/// Alias + name matching (high precision): a node matches when every query token is a
/// substring of its `name:` or one of its `aliases:` (design.md §8/§9).
fn alias_pass(
    index: &Index,
    tokens: &[String],
    acc: &mut std::collections::BTreeMap<String, Acc>,
) -> Result<()> {
    let rows = index.query_rows("SELECT id, type, path, frontmatter FROM nodes", (), |r| {
        Ok((
            col_text(r, 0)?,
            col_text(r, 1)?,
            col_text(r, 2)?,
            col_text(r, 3)?,
        ))
    })?;
    // Nodes that alias-match but have no anchor yet need a fallback first-section anchor.
    // Collect them, then fetch all their first sections in one query (avoids an N+1 of
    // per-node round-trips through the block_on facade).
    let mut needs_anchor: Vec<String> = Vec::new();
    for (id, node_type, path, fm) in rows {
        let json: serde_json::Value = serde_json::from_str(&fm).unwrap_or(serde_json::Value::Null);
        let mut candidates: Vec<String> = Vec::new();
        if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
            candidates.push(name.to_lowercase());
        }
        if let Some(aliases) = json.get("aliases").and_then(|v| v.as_array()) {
            for a in aliases {
                if let Some(s) = a.as_str() {
                    candidates.push(s.to_lowercase());
                }
            }
        }
        let matched = candidates
            .iter()
            .any(|c| tokens.iter().all(|t| c.contains(t.as_str())));
        if !matched {
            continue;
        }
        let entry = acc.entry(id.clone()).or_insert_with(|| Acc {
            node_type,
            path,
            score: 0.0,
            anchors: Default::default(),
        });
        entry.score += ALIAS_WEIGHT;
        // An earlier FTS pass may already have anchored this node; only alias-only matches
        // (no lexical prose hit) fall back to the first section.
        if entry.anchors.is_empty() {
            needs_anchor.push(id);
        }
    }

    for (id, heading, line, body) in first_sections(index, &needs_anchor)? {
        if let Some(entry) = acc.get_mut(&id)
            && entry.anchors.is_empty()
        {
            entry.anchors.insert(
                line,
                Anchor {
                    heading,
                    line,
                    snippet: snippet(&body, tokens),
                },
            );
        }
    }
    Ok(())
}

/// Vector recall: embed the query and add nodes whose best section exceeds the cosine
/// threshold. The recall layer behind FTS + aliases (design.md §9). Cosine is now computed
/// in the engine via Turso's native `vector_distance_cos` (which returns a *distance*, so
/// `similarity = 1 - distance`); the hand-rolled brute-force loop is retired.
fn vector_pass(
    index: &Index,
    embedder: &dyn Embedder,
    query: &str,
    tokens: &[String],
    acc: &mut std::collections::BTreeMap<String, Acc>,
) -> Result<()> {
    let qvec = match embedder.embed(&[query.to_string()])?.into_iter().next() {
        Some(v) => v,
        None => return Ok(()),
    };
    // A zero query vector has an undefined cosine; skip the pass rather than error in SQL.
    if qvec.iter().all(|x| *x == 0.0) {
        return Ok(());
    }
    // The query vector as a little-endian f32 blob — Turso reads it as a Float32-dense
    // vector directly (same layout the stored `vector` column uses).
    let qblob = vector::encode_vector(&qvec);
    // `vector_distance_cos` hard-errors on a dimension mismatch, so only compare against
    // stored vectors of the same width (byte length ⇒ f32 count ⇒ dims). This makes a
    // stale-dimension index — one built before an embedding-dimensions change and not yet
    // `--re-embed`ed — degrade to "no vector recall" instead of failing the query, matching
    // the old brute-force cosine, which returned 0 similarity for mismatched lengths.
    let qlen = qblob.len() as i64;

    let rows = index.query_rows(
        "SELECT e.node_id, e.section_line,
                vector_distance_cos(e.vector, ?1) AS dist,
                n.type, n.path, s.heading, s.body
         FROM embeddings e
         JOIN nodes n ON n.id = e.node_id
         JOIN sections s ON s.node_id = e.node_id AND s.line = e.section_line
         WHERE length(e.vector) = ?2",
        turso::params![qblob, qlen],
        |r| {
            Ok((
                col_text(r, 0)?,
                col_u32(r, 1)?,
                col_f64(r, 2)?,
                col_text(r, 3)?,
                col_text(r, 4)?,
                col_text(r, 5)?,
                col_text(r, 6)?,
            ))
        },
    )?;
    for (id, line, dist, node_type, path, heading, body) in rows {
        // A zero-magnitude stored vector (e.g. an empty section) gives an undefined cosine,
        // which Turso returns as NaN. Skip it rather than let NaN poison the score — matching
        // the old brute-force cosine, which returned 0 similarity for a zero vector.
        if !dist.is_finite() {
            continue;
        }
        let sim = 1.0 - dist as f32;
        if sim < VECTOR_THRESHOLD {
            continue;
        }
        let entry = acc.entry(id).or_insert_with(|| Acc {
            node_type,
            path,
            score: 0.0,
            anchors: Default::default(),
        });
        entry.score += VECTOR_WEIGHT * sim;
        entry.anchors.entry(line).or_insert_with(|| Anchor {
            heading,
            line,
            snippet: snippet(&body, tokens),
        });
    }
    Ok(())
}

/// One `suggest` candidate: an existing node a descriptor might refer to.
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub id: NodeId,
    pub node_type: NodeType,
    pub name: String,
    pub path: String,
    pub score: f32,
}

// Suggestion scoring: an exact name/alias match beats a token-subset match, both beat a
// prose-only (FTS backup) hit (design.md §8: alias + FTS first).
const SUGGEST_ALIAS_EXACT: f32 = 3.0;
const SUGGEST_ALIAS_TOKENS: f32 = 2.0;
const SUGGEST_FTS_BONUS: f32 = 0.5;

/// Suggest existing nodes a `descriptor` might refer to, ranked — the lookup-before-
/// reference primitive (design.md §7/§8). Matches the descriptor against each node's
/// `name`/`aliases` (high precision), with prose FTS as a backup. No vectors: bare
/// embeddings are weak for short descriptors (§9). Sorted by score desc, then `id`.
pub fn suggest(
    index: &Index,
    descriptor: &str,
    type_filter: Option<&NodeType>,
    limit: usize,
) -> Result<Vec<Suggestion>> {
    let needle = descriptor.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let tokens = tokenize(descriptor);

    // (type, path, display name) for every node, plus accumulated scores.
    let mut info: std::collections::HashMap<String, (String, String, String)> =
        std::collections::HashMap::new();
    let mut scores: std::collections::HashMap<String, f32> = std::collections::HashMap::new();

    let rows = index.query_rows("SELECT id, type, path, frontmatter FROM nodes", (), |r| {
        Ok((
            col_text(r, 0)?,
            col_text(r, 1)?,
            col_text(r, 2)?,
            col_text(r, 3)?,
        ))
    })?;
    for (id, node_type, path, fm) in rows {
        let json: serde_json::Value = serde_json::from_str(&fm).unwrap_or(serde_json::Value::Null);
        let name = json
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut candidates: Vec<String> = Vec::new();
        if !name.is_empty() {
            candidates.push(name.to_lowercase());
        }
        if let Some(arr) = json.get("aliases").and_then(|v| v.as_array()) {
            for a in arr {
                if let Some(s) = a.as_str() {
                    candidates.push(s.to_lowercase());
                }
            }
        }
        let mut s = 0.0f32;
        for c in &candidates {
            if *c == needle {
                s = s.max(SUGGEST_ALIAS_EXACT);
            } else if !tokens.is_empty() && tokens.iter().all(|t| c.contains(t.as_str())) {
                s = s.max(SUGGEST_ALIAS_TOKENS);
            }
        }
        if s > 0.0 {
            *scores.entry(id.clone()).or_insert(0.0) += s;
        }
        info.insert(id, (node_type, path, name));
    }

    // FTS backup: nodes whose prose matches the descriptor.
    if !tokens.is_empty() {
        let match_query = tokens.join(" ");
        let ids = index.query_rows(
            "SELECT DISTINCT node_id FROM sections WHERE fts_match(heading, body, ?1)",
            [match_query.as_str()],
            |r| col_text(r, 0),
        )?;
        for id in ids {
            if info.contains_key(&id) {
                *scores.entry(id).or_insert(0.0) += SUGGEST_FTS_BONUS;
            }
        }
    }

    let mut out: Vec<Suggestion> = scores
        .into_iter()
        .filter(|(_, s)| *s > 0.0)
        .filter_map(|(id, score)| {
            let (node_type, path, name) = info.get(&id)?.clone();
            let node_type = NodeType::new(node_type);
            if let Some(t) = type_filter
                && &node_type != t
            {
                return None;
            }
            Some(Suggestion {
                id: id.parse().ok()?,
                node_type,
                name,
                path,
                score,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    out.truncate(limit);
    Ok(out)
}

/// Node IDs scoped to `project` via a `project` edge (cli.md §3.4 `--scope`).
fn scope_set(
    index: &Index,
    container: &NodeId,
    scope_field: &str,
) -> Result<std::collections::HashSet<String>> {
    let rows = index.query_rows(
        "SELECT from_id FROM edges WHERE ref_type = ?1 AND to_id = ?2",
        turso::params![scope_field, container.to_string()],
        |r| col_text(r, 0),
    )?;
    Ok(rows.into_iter().collect())
}

/// The first section (lowest line) of each of `node_ids`, for a fallback anchor when
/// nothing else matched — fetched in a single query. Returns one `(node_id, heading, line,
/// body)` per node that has any section.
fn first_sections(
    index: &Index,
    node_ids: &[String],
) -> Result<Vec<(String, String, u32, String)>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (1..=node_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let params: Vec<turso::Value> = node_ids
        .iter()
        .map(|id| turso::Value::from(id.clone()))
        .collect();
    let sql = format!(
        "SELECT node_id, heading, line, body FROM sections
         WHERE node_id IN ({placeholders}) ORDER BY node_id, line"
    );
    let rows = index.query_rows(&sql, params, |r| {
        Ok((
            col_text(r, 0)?,
            col_text(r, 1)?,
            col_u32(r, 2)?,
            col_text(r, 3)?,
        ))
    })?;
    // The sections are ordered by (node_id, line), so keep the first seen for each node_id.
    let mut out: Vec<(String, String, u32, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (id, heading, line, body) in rows {
        if seen.insert(id.clone()) {
            out.push((id, heading, line, body));
        }
    }
    Ok(out)
}

/// Lowercase alphanumeric tokens, de-duplicated, order-preserving.
fn tokenize(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// How many times any query token occurs (case-insensitively) in `body`.
fn term_frequency(body: &str, tokens: &[String]) -> usize {
    let lower = body.to_lowercase();
    tokens
        .iter()
        .map(|t| lower.matches(t.as_str()).count())
        .sum()
}

/// A short, whitespace-collapsed snippet windowed around the first matching token.
fn snippet(body: &str, tokens: &[String]) -> String {
    let words: Vec<&str> = body.split_whitespace().collect();
    let hit = words
        .iter()
        .position(|w| tokens.iter().any(|t| w.to_lowercase().contains(t.as_str())));
    match hit {
        Some(i) => {
            let start = i.saturating_sub(4);
            let end = (i + 8).min(words.len());
            words[start..end].join(" ")
        }
        None => words.iter().take(12).copied().collect::<Vec<_>>().join(" "),
    }
}
