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
use crate::index::db::Index;
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
    let conn = index.conn();
    let mut acc: std::collections::BTreeMap<String, Acc> = std::collections::BTreeMap::new();

    fts_pass(conn, &tokens, &mut acc)?;
    alias_pass(conn, &tokens, &mut acc)?;
    vector_pass(conn, embedder, query, &tokens, &mut acc)?;

    // Filters.
    if let Some(t) = &opts.type_filter {
        acc.retain(|_, a| a.node_type == t.as_str());
    }
    if let Some(scope) = &opts.scope {
        let in_scope = scope_set(conn, scope, &opts.scope_field)?;
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

/// FTS5 over section bodies + headings. Candidate sections are found via `MATCH`, then
/// scored by query-term frequency in Rust (transparent and bm25-sign-agnostic).
fn fts_pass(
    conn: &rusqlite::Connection,
    tokens: &[String],
    acc: &mut std::collections::BTreeMap<String, Acc>,
) -> Result<()> {
    let match_expr = tokens
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ");

    let mut stmt = conn.prepare(
        "SELECT sections_fts.node_id, sections_fts.heading, sections_fts.line, sections_fts.body,
                nodes.type, nodes.path
         FROM sections_fts JOIN nodes ON nodes.id = sections_fts.node_id
         WHERE sections_fts MATCH ?1",
    )?;
    let rows = stmt.query_map([&match_expr], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u32>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    for row in rows {
        let (id, heading, line, body, node_type, path) = row?;
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
    conn: &rusqlite::Connection,
    tokens: &[String],
    acc: &mut std::collections::BTreeMap<String, Acc>,
) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id, type, path, frontmatter FROM nodes")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, node_type, path, fm) = row?;
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
        if entry.anchors.is_empty()
            && let Some((heading, line, body)) = first_section(conn, &id)?
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
/// threshold. The recall layer behind FTS + aliases (design.md §9).
fn vector_pass(
    conn: &rusqlite::Connection,
    embedder: &dyn Embedder,
    query: &str,
    tokens: &[String],
    acc: &mut std::collections::BTreeMap<String, Acc>,
) -> Result<()> {
    let qvec = match embedder.embed(&[query.to_string()])?.into_iter().next() {
        Some(v) => v,
        None => return Ok(()),
    };

    let mut stmt = conn.prepare(
        "SELECT e.node_id, e.section_line, e.vector, nodes.type, nodes.path,
                sections_fts.heading, sections_fts.body
         FROM embeddings e
         JOIN nodes ON nodes.id = e.node_id
         JOIN sections_fts ON sections_fts.node_id = e.node_id AND sections_fts.line = e.section_line",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, u32>(1)?,
            r.get::<_, Vec<u8>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    for row in rows {
        let (id, line, blob, node_type, path, heading, body) = row?;
        let sim = vector::cosine(&qvec, &vector::decode_vector(&blob));
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
    let conn = index.conn();
    let needle = descriptor.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let tokens = tokenize(descriptor);

    // (type, path, display name) for every node, plus accumulated scores.
    let mut info: std::collections::HashMap<String, (String, String, String)> =
        std::collections::HashMap::new();
    let mut scores: std::collections::HashMap<String, f32> = std::collections::HashMap::new();

    let mut stmt = conn.prepare("SELECT id, type, path, frontmatter FROM nodes")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, node_type, path, fm) = row?;
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
        let match_expr = tokens
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut stmt =
            conn.prepare("SELECT DISTINCT node_id FROM sections_fts WHERE sections_fts MATCH ?1")?;
        let rows = stmt.query_map([&match_expr], |r| r.get::<_, String>(0))?;
        for row in rows {
            let id = row?;
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
    conn: &rusqlite::Connection,
    container: &NodeId,
    scope_field: &str,
) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT from_id FROM edges WHERE ref_type = ?1 AND to_id = ?2")?;
    let rows = stmt.query_map(rusqlite::params![scope_field, container.to_string()], |r| {
        r.get::<_, String>(0)
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The first section of a node (lowest line), for an anchor when nothing else matched.
fn first_section(
    conn: &rusqlite::Connection,
    node_id: &str,
) -> Result<Option<(String, u32, String)>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT heading, line, body FROM sections_fts WHERE node_id = ?1 ORDER BY line LIMIT 1",
            [node_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
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
