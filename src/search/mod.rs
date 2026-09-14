//! Hybrid retrieval — FTS first, vectors for recall (design.md §9, cli.md §3.4).
//!
//! Three retrieval jobs want different things; this module serves open-ended `search`:
//! FTS + `aliases:` carry precision, vectors are the recall layer behind them. The
//! **file is the returned unit**, with the matching section anchors. Results sort by
//! descending score; ties break by `id` ascending for determinism.
//!
//! Three signals feed the final ranking, each scored by its own pass and kept
//! architecturally separate from how they combine:
//! - **lexical** ([`lexical_candidates`]) — native Tantivy BM25 per section (`fts_score`),
//!   MAX-aggregated per file. Deliberately the one function that knows how section matches
//!   become a file score + ranked anchor sections, so a different lexical scorer drops in
//!   by replacing it alone.
//! - **name** ([`name_pass`]) — graded name/alias/id-slug tiers (design.md §8), the same
//!   precision `suggest` already has.
//! - **vector** ([`vector_pass`]) — brute-force cosine top-K, a noise floor rather than a
//!   precision gate (a fixed high cosine threshold routinely returns *zero* sections for a
//!   real embedder, so vectors never influence ranking at all).
//!
//! [`fuse`] is what turns those three signals into the single score results are ordered
//! by: reciprocal-rank fusion over lexical + name + vector, with a node's name tier able to
//! hard-outrank a lower tier's regardless of its lexical/vector standing once that tier is
//! strong enough to trust absolutely — see [`fuse`] for exactly where that line is drawn
//! and why.
//!
//! (Reference resolution, design.md §8, is the *same* machinery used in the other
//! direction — alias + FTS first, embeddings as backup — and will live alongside this.)

pub mod vector;

use std::collections::{BTreeMap, HashMap, HashSet};

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

// ---- signal + fusion constants (opaque relative ranks, design.md §9 / cli.md §3.4) ----

/// How many candidate sections `fts_match`/`fts_score` considers, ranked by score, before
/// per-file MAX aggregation. Bounding this bounds the follow-up [`node_meta`] lookup to a
/// fixed-size candidate set instead of however many sections in the whole corpus matched.
/// Chosen on the search benchmark: quality-identical to a much larger/unlimited cap, but
/// meaningfully faster on a large corpus.
const FTS_CANDIDATE_CAP: Option<i64> = Some(100);

/// How many sections the vector pass pulls, ranked by raw cosine distance. No dense-vector
/// index exists, so this stays a brute-force `ORDER BY dist LIMIT`; this bounds that scan.
const VECTOR_TOP_K: i64 = 20;
/// Minimum cosine similarity for a file to count as vector-recalled at all. Far below a
/// fixed high-confidence gate, but not zero: with no floor at all, [`VECTOR_TOP_K`]
/// sections are pulled for *every* query regardless of relevance (a brute-force top-K has
/// no "no match" outcome), which would make even a nonsense query surface filler results.
/// This is a noise floor, not a precision gate — precision still comes from lexical +
/// name. Chosen on the search benchmark.
const VECTOR_MIN_SIM: f32 = 0.3;

/// Reciprocal-rank fusion (see [`fuse`]): `score = Σ w_s / (RRF_K + rank_s)` over each
/// signal a file appears in. RRF's rank-damping constant, chosen on the search benchmark:
/// a small value lets 1st place lead more decisively, which suits how clean each signal's
/// own ranking already is here (each is either a real match or absent, not a noisy
/// retriever).
const RRF_K: f32 = 5.0;
/// Vector's RRF weight, at parity with lexical (whose own term is unweighted, `1.0`).
/// Chosen on the search benchmark.
const RRF_W_VEC: f32 = 1.0;
/// The name signal's RRF weight — folded in alongside lexical/vector *before* the
/// [`NAME_GATE_TIER`] hard gate is applied (see [`fuse`]), so a name match below the gate
/// still nudges the ranking instead of contributing nothing. Chosen on the search
/// benchmark: too heavy a weight lets a weak (tier 1) partial-token match crowd out a
/// genuinely better lexical/vector candidate in the RRF sum.
const RRF_W_NAME: f32 = 2.0;
/// The name/alias/id-slug tier (see [`name_match_tier`]) at or above which a match hard-
/// outranks every file below that tier, regardless of lexical/vector standing. Chosen on
/// the search benchmark: gating tier 2 ("candidate's tokens subset of the query's", e.g. a
/// short name inside a long question) too aggressively hard-outranked some genuinely
/// better `document`/`question` matches. Tiers 1-2 still feed the fused score via
/// [`RRF_W_NAME`], just without the hard gate.
const NAME_GATE_TIER: u8 = 3;

/// Separator between the display name and each alias in `nodes.alias_text`.
///
/// U+001F (unit separator), deliberately not NUL: SQLite's `LIKE` stops at an embedded
/// NUL, which would hide every alias but the first from the narrowing filter.
pub const ALIAS_SEP: char = '\u{1f}';
/// Cap anchors reported per file, so output stays readable.
const MAX_ANCHORS: usize = 3;

/// One node accumulating its per-signal scores and matched section candidates across the
/// three passes. Nothing here is a final rank until [`fuse`] runs.
struct Acc {
    node_type: String,
    path: String,
    /// [`lexical_candidates`]'s per-file score: BM25 MAX over matching sections.
    lexical_score: f32,
    /// That file's best-scoring sections, `(line, heading)`, best first, capped at
    /// [`MAX_ANCHORS`] — bodies are fetched later, only for files that make the final cut
    /// (see `resolve_anchors`).
    lexical_sections: Vec<(u32, String)>,
    /// The node's best name/alias/id-slug match tier (`0` = no match; see
    /// [`name_match_tier`]), across all its candidates.
    name_tier: u8,
    /// Best cosine similarity among this file's sections in the vector pass's top-K set,
    /// or `0.0` if it didn't appear there.
    vector_sim: f32,
    /// The section line of that best similarity. Heading/body are fetched later, only for
    /// files that make the final cut (see `resolve_anchors`).
    vector_line: Option<u32>,
    /// Set by [`fuse`]: the combined score results are ranked by.
    score: f32,
    /// Set by [`fuse`]: whether the vector signal contributed more than lexical to
    /// `score`, so anchors are ordered by whichever signal actually mattered here.
    vector_leads: bool,
    /// Final anchors, resolved once ranking has settled (see `resolve_anchors`).
    anchors: Vec<Anchor>,
}

impl Acc {
    fn new(node_type: String, path: String) -> Self {
        Acc {
            node_type,
            path,
            lexical_score: 0.0,
            lexical_sections: Vec::new(),
            name_tier: 0,
            vector_sim: 0.0,
            vector_line: None,
            score: 0.0,
            vector_leads: false,
            anchors: Vec::new(),
        }
    }
}

/// Run hybrid search. Lexical + name matches are scored first (precision); the vector pass
/// (brute-force cosine over the embeddings blob) adds recall; [`fuse`] combines the three
/// signals' own ranked lists into one score, and results are ranked by that score
/// descending, ties broken by `id` ascending (cli.md §3.4).
pub fn search(
    index: &Index,
    embedder: &dyn Embedder,
    query: &str,
    opts: &SearchOpts,
) -> Result<Vec<SearchHit>> {
    let qvec = embed_query(embedder, query)?;
    search_prepared(index, qvec.as_deref(), query, opts)
}

/// Embed the query once (the blob is reused across every member in a workspace search).
/// `None` when the embedder returns nothing or a zero vector (undefined cosine).
fn embed_query(embedder: &dyn Embedder, query: &str) -> Result<Option<Vec<f32>>> {
    let Some(qvec) = embedder.embed(&[query.to_string()])?.into_iter().next() else {
        return Ok(None);
    };
    if qvec.iter().all(|x| *x == 0.0) {
        return Ok(None);
    }
    Ok(Some(qvec))
}

/// [`search`] against one index with an already-embedded query vector.
pub fn search_prepared(
    index: &Index,
    qvec: Option<&[f32]>,
    query: &str,
    opts: &SearchOpts,
) -> Result<Vec<SearchHit>> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();

    for (id, m) in lexical_candidates(index, &tokens)? {
        let entry = acc
            .entry(id)
            .or_insert_with(|| Acc::new(m.node_type, m.path));
        entry.lexical_score = m.score;
        entry.lexical_sections = m.sections;
    }
    name_pass(index, &tokens, &mut acc)?;
    if let Some(qvec) = qvec {
        vector_pass(index, qvec, VECTOR_TOP_K, &mut acc)?;
    }
    fuse(&mut acc);

    // Filters.
    if let Some(t) = &opts.type_filter {
        acc.retain(|_, a| a.node_type == t.as_str());
    }
    if let Some(scope) = &opts.scope {
        let in_scope = scope_set(index, scope, &opts.scope_field)?;
        acc.retain(|id, _| in_scope.contains(id));
    }

    // Rank + truncate *before* resolving anchors, so the small DB lookups in
    // `resolve_anchors` only ever touch the files that actually make the final cut.
    let mut hits: Vec<(String, Acc)> = acc.into_iter().collect();
    hits.sort_by(|(xid, x), (yid, y)| {
        y.score
            .partial_cmp(&x.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| xid.cmp(yid))
    });
    hits.truncate(opts.limit.unwrap_or(10));

    resolve_anchors(index, &tokens, &mut hits)?;

    Ok(hits
        .into_iter()
        .map(|(id, a)| SearchHit {
            id: NodeId::parse_stored(&id),
            node_type: NodeType::new(a.node_type),
            path: a.path,
            score: a.score,
            anchors: a.anchors,
        })
        .collect())
}

/// One node's lexical (BM25) match (see [`lexical_candidates`]): the file-level score (MAX
/// aggregation over its matched sections) and up to [`MAX_ANCHORS`] of its best-scoring
/// sections, `(line, heading)`, best first.
struct LexicalMatch {
    node_type: String,
    path: String,
    score: f32,
    sections: Vec<(u32, String)>,
}

/// The lexical signal: native Tantivy BM25 scoring per section (`fts_score`), MAX
/// aggregation per file — a long document no longer wins just by having more matching
/// sections, over a bounded candidate set, with anchors picked by score rather than line
/// position.
///
/// Deliberately the ONE function that knows how section-level matches become a file-level
/// lexical score plus ranked anchor sections: everything downstream ([`fuse`],
/// `resolve_anchors`) only ever reads a [`LexicalMatch`], so a different lexical scorer
/// (e.g. these same candidate sections re-scored in Rust) drops in by replacing this one
/// function.
///
/// The ranked candidate statement ([`fts_pass_candidates`]) is one of the two patterns
/// Tantivy's ranked optimizer path actually matches: no JOIN, the same `?1` bound to both
/// `fts_match` and `fts_score`, and — when capped, as here — a plain integer `LIMIT`. A
/// JOIN or an extra `ORDER BY` key silently drops to the *fallback* pattern, which returns
/// `0.0` for every row with no error, so `node_type`/`path` are fetched by a separate query
/// ([`node_meta`]) instead of being joined into this one, and section bodies are deferred
/// further still, to `resolve_anchors` — only for the anchors of nodes that actually
/// survive to the final hit list, never this pass's whole (already capped) candidate set.
///
/// `heading` already carries the index's `weights='heading=2.0,body=1.0'` boost inside
/// `fts_score`, so a heading-only match scores instead of being skipped entirely.
fn lexical_candidates(index: &Index, tokens: &[String]) -> Result<BTreeMap<String, LexicalMatch>> {
    let match_query = tokens.join(" ");
    let mut rows = fts_pass_candidates(index, &match_query, FTS_CANDIDATE_CAP)?;
    if !rows.is_empty() && rows.iter().all(|(.., score)| *score == 0.0) {
        // Every fts_match'd row scored exactly 0.0: the ranked statement's optimizer
        // pattern did not match and Turso silently fell back to its ranking-blind path.
        // Loud in debug/test builds; degrade gracefully in release rather than silently
        // serve unranked results with no indication anything is wrong.
        debug_assert!(
            false,
            "fts_score returned 0.0 for every fts_match row — the ranked Tantivy query \
             pattern likely stopped matching; falling back to a plain token-overlap score"
        );
        rows = fts_pass_fallback(index, &match_query, tokens)?;
    }
    // Deterministic order for both the per-file aggregation and anchor selection: score
    // descending, ties by line ascending — resolved in Rust so it holds whether or not the
    // SQL itself applied an ORDER BY (an uncapped `FTS_CANDIDATE_CAP` would omit one).
    rows.sort_by(|a, b| {
        b.3.partial_cmp(&a.3)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.2.cmp(&b.2))
    });

    let mut per_node: BTreeMap<String, RawLexicalMatch> = BTreeMap::new();
    for (id, heading, line, score) in rows {
        let raw = per_node.entry(id).or_default();
        raw.scores.push(score);
        // `rows` is already sorted best-first, so the first `MAX_ANCHORS` sections seen
        // for a node are its best-matching ones — not, as before, whichever happened to
        // come first by line position.
        if raw.sections.len() < MAX_ANCHORS {
            raw.sections.push((line, heading));
        }
    }
    if per_node.is_empty() {
        return Ok(BTreeMap::new());
    }

    // type/path for every matched node in one query — never JOINed into the ranked
    // statement above (that disables `fts_score`).
    let ids: Vec<String> = per_node.keys().cloned().collect();
    let meta = node_meta(index, &ids)?;

    Ok(per_node
        .into_iter()
        .filter_map(|(id, raw)| {
            let (node_type, path) = meta.get(&id)?.clone();
            Some((
                id,
                LexicalMatch {
                    node_type,
                    path,
                    // MAX aggregation: the single best-matching section's score, not the
                    // sum over every one — `scores` is sorted best-first.
                    score: raw.scores.first().copied().unwrap_or(0.0),
                    sections: raw.sections,
                },
            ))
        })
        .collect())
}

/// [`lexical_candidates`]'s per-node accumulator while section rows are still being
/// folded in: raw matched-section scores (for MAX aggregation) and best-scoring sections
/// (for anchors), before node metadata ([`node_meta`]) is known.
#[derive(Default)]
struct RawLexicalMatch {
    scores: Vec<f32>,
    sections: Vec<(u32, String)>,
}

/// The ranked candidate statement: no JOIN, the same `?1` bound to both `fts_match` and
/// `fts_score`, a plain integer `LIMIT` when capped. `cap = None` uses the other verified
/// shape (no `ORDER BY`/`LIMIT` at all — sorted client-side by the caller).
fn fts_pass_candidates(
    index: &Index,
    match_query: &str,
    cap: Option<i64>,
) -> Result<Vec<(String, String, u32, f32)>> {
    match cap {
        Some(limit) => index.query_rows(
            "SELECT node_id, heading, line, fts_score(heading, body, ?1) AS score
             FROM sections WHERE fts_match(heading, body, ?1)
             ORDER BY score DESC LIMIT ?2",
            turso::params![match_query, limit],
            candidate_row,
        ),
        None => index.query_rows(
            "SELECT node_id, heading, line, fts_score(heading, body, ?1) AS score
             FROM sections WHERE fts_match(heading, body, ?1)",
            [match_query],
            candidate_row,
        ),
    }
}

/// Row mapper shared by both [`fts_pass_candidates`] statement shapes.
fn candidate_row(r: &turso::Row) -> Result<(String, String, u32, f32)> {
    Ok((
        col_text(r, 0)?,
        col_text(r, 1)?,
        col_u32(r, 2)?,
        col_f64(r, 3)? as f32,
    ))
}

/// Safety net for the silent-fallback hazard [`lexical_candidates`] guards against: a plain
/// whole-token overlap count per section (never a substring match — "end" must not match
/// "endpoint"), so a release build still serves *some* ranked results instead of the
/// ranking-blind order Turso's own fallback would otherwise leave in place with no
/// indication anything is wrong. Proven not to be needed against the production statement
/// by the `fts_score_returns_real_varying_scores` test below; not expected to ever run
/// against production Turso/Tantivy.
fn fts_pass_fallback(
    index: &Index,
    match_query: &str,
    tokens: &[String],
) -> Result<Vec<(String, String, u32, f32)>> {
    let rows = index.query_rows(
        "SELECT node_id, heading, line, body FROM sections WHERE fts_match(heading, body, ?1)",
        [match_query],
        |r| {
            Ok((
                col_text(r, 0)?,
                col_text(r, 1)?,
                col_u32(r, 2)?,
                col_text(r, 3)?,
            ))
        },
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, heading, line, body)| {
            // Mirror the index's heading=2.0/body=1.0 field weights so the fallback's
            // ordering at least loosely resembles the BM25 shape it stands in for.
            let score =
                token_overlap(&heading, tokens) as f32 * 2.0 + token_overlap(&body, tokens) as f32;
            (score > 0.0).then_some((id, heading, line, score))
        })
        .collect())
}

/// How many distinct query tokens occur as a *whole* token in `text` (never a substring —
/// "end" must not match "endpoint").
fn token_overlap(text: &str, tokens: &[String]) -> usize {
    let text_tokens = tokenize(text);
    tokens.iter().filter(|t| text_tokens.contains(t)).count()
}

/// `(type, path)` for each of `ids`, in one query — the ranked candidate statement above
/// cannot JOIN `nodes` without disabling `fts_score`.
fn node_meta(index: &Index, ids: &[String]) -> Result<HashMap<String, (String, String)>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let (placeholders, params) = in_list_params(ids, 1);
    let sql = format!("SELECT id, type, path FROM nodes WHERE id IN ({placeholders})");
    let rows = index.query_rows(&sql, params, |r| {
        Ok((col_text(r, 0)?, col_text(r, 1)?, col_text(r, 2)?))
    })?;
    Ok(rows
        .into_iter()
        .map(|(id, node_type, path)| (id, (node_type, path)))
        .collect())
}

/// A small English stopword list for the name-match signal only (design.md §8/§9) — never
/// used by [`lexical_candidates`] or [`snippet`]. Function words shouldn't count as
/// meaningful overlap when a query is graded against a node's name/alias/id-slug
/// candidates: without this, a query like "what is a loose end" would share a "token" with
/// almost every node in the corpus.
const NAME_STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "at", "to", "for", "and", "or", "is", "are", "was", "were",
    "be", "been", "being", "by", "with", "as", "that", "this", "these", "those", "it", "its",
    "from", "what", "who", "whom", "which", "how", "why", "when", "where", "does", "do", "did",
    "has", "have", "had", "can", "will", "would", "should", "could", "about", "into", "than",
    "then", "there", "their", "you", "your", "i", "we", "they",
];

fn is_name_stopword(token: &str) -> bool {
    NAME_STOPWORDS.contains(&token)
}

/// Graded name/alias/id-slug matching (design.md §8/§9) — gives `search` the precision
/// `suggest` already has. A node's candidates are its display name, every alias (from
/// `alias_text`), and its own id slug with `-`/`_` turned into spaces (scope and `type:`
/// stripped, e.g. `principle:gate-the-rare-act` → "gate the rare act"). The node's tier is
/// the best over all candidates ([`node_name_tier`]); [`fuse`] folds it into the node's
/// score once every pass has run — this pass only records the tier, no anchors: a
/// name-only match (no lexical or vector hit) gets its fallback anchor centrally, in
/// `resolve_anchors`, for whichever such nodes survive to the final hit list — never
/// eagerly for this pass's whole (possibly much wider) candidate set.
///
/// Would also improve `suggest` (unchanged per this task): its exact/token-subset match is
/// a coarser 2-tier version of the same idea, and it has no id-slug candidate.
fn name_pass(index: &Index, tokens: &[String], acc: &mut BTreeMap<String, Acc>) -> Result<()> {
    // Narrow the scan with the query's non-stopword tokens: any candidate a token matches
    // as a *whole token* is necessarily also a *substring* of `alias_text` (each candidate
    // is one of its unit-separated segments, copied verbatim) and of the raw `id` (tokens
    // are alphanumeric-only, so they survive the id's `-`/`_` → space rewrite unchanged) —
    // so this narrowing cannot miss a real match. Stopwords are excluded because a short
    // one (e.g. "a") is a substring of almost every row and would narrow nothing; if every
    // token is a stopword this falls back to a full scan, which is still just the whole
    // (small) `nodes` table.
    let narrow: Vec<&String> = tokens.iter().filter(|t| !is_name_stopword(t)).collect();
    let (sql, params) = name_narrowing_query(&narrow);
    let rows = index.query_rows(&sql, params, |r| {
        Ok((
            col_text(r, 0)?,
            col_text(r, 1)?,
            col_text(r, 2)?,
            col_text(r, 3)?,
        ))
    })?;
    for (id, node_type, path, alias_text) in rows {
        let tier = node_name_tier(tokens, &id, &alias_text);
        if tier == 0 {
            continue;
        }
        let entry = acc.entry(id).or_insert_with(|| Acc::new(node_type, path));
        entry.name_tier = tier;
    }
    Ok(())
}

/// Build [`name_pass`]'s scan: an OR of `alias_text LIKE` / `id LIKE` per non-stopword
/// token (narrow), or an unfiltered scan of `nodes` when every query token is a stopword.
fn name_narrowing_query(narrow_tokens: &[&String]) -> (String, Vec<turso::Value>) {
    if narrow_tokens.is_empty() {
        return (
            "SELECT id, type, path, alias_text FROM nodes".to_string(),
            Vec::new(),
        );
    }
    let clauses: Vec<String> = (1..=narrow_tokens.len())
        .map(|i| format!("alias_text LIKE ?{i} ESCAPE '\\' OR id LIKE ?{i} ESCAPE '\\'"))
        .collect();
    let sql = format!(
        "SELECT id, type, path, alias_text FROM nodes WHERE {}",
        clauses.join(" OR ")
    );
    let params = narrow_tokens
        .iter()
        .map(|t| turso::Value::from(format!("%{}%", like_escape(t))))
        .collect();
    (sql, params)
}

/// One node's best match tier over its name/alias/id-slug candidates — the max over
/// [`name_match_tier`] applied to the display name, every alias, and the id slug (scope
/// and `type:` stripped, `-`/`_` → space; e.g. `principle:gate-the-rare-act` → "gate the
/// rare act").
fn node_name_tier(tokens: &[String], id: &str, alias_text: &str) -> u8 {
    let slug = NodeId::parse_stored(id)
        .local()
        .replace(['-', '_'], " ")
        .to_lowercase();
    alias_text
        .split(ALIAS_SEP)
        .filter(|c| !c.is_empty())
        .chain(std::iter::once(slug.as_str()))
        .map(|candidate| name_match_tier(tokens, &tokenize(candidate)))
        .max()
        .unwrap_or(0)
}

/// One candidate's graded match tier against the query's tokens (design.md §8) — gives
/// `search` the name/alias precision `suggest` already has:
/// - 4 — exact: the query's token set equals the candidate's.
/// - 3 — every query token is present in the candidate (the query sits inside a longer
///   name, e.g. "logistics" in "logistics contact").
/// - 2 — the candidate's tokens all sit inside the query (the name sits inside a longer
///   query, e.g. "loose end" in "what is a loose end"), and the candidate has at least one
///   non-stopword token (so an all-stopword candidate can't piggyback on this branch).
/// - 1 — partial: at least one shared non-stopword token.
/// - 0 — no match.
///
/// [`fuse`] hard-gates tiers >= [`NAME_GATE_TIER`] and folds every nonzero tier into the
/// fused score via [`RRF_W_NAME`] — see there for why tier 1 alone does not hard-gate.
fn name_match_tier(query_tokens: &[String], candidate_tokens: &[String]) -> u8 {
    if candidate_tokens.is_empty() {
        return 0;
    }
    let q: HashSet<&str> = query_tokens.iter().map(String::as_str).collect();
    let c: HashSet<&str> = candidate_tokens.iter().map(String::as_str).collect();
    if q == c {
        return 4;
    }
    if q.is_subset(&c) {
        return 3;
    }
    if c.is_subset(&q) && c.iter().any(|t| !is_name_stopword(t)) {
        return 2;
    }
    let has_shared_non_stop = q
        .iter()
        .copied()
        .filter(|t| !is_name_stopword(t))
        .any(|t| c.contains(t));
    if has_shared_non_stop { 1 } else { 0 }
}

/// Vector recall: brute-force top-K nearest sections by cosine distance — no threshold (a
/// fixed high cosine gate means a real embedder essentially never contributes). No JOIN:
/// type/path for genuinely new candidates and heading/body for anchors are fetched
/// separately (`nodes_info`, `resolve_anchors`), so this scan never pulls prose or node
/// metadata for the ~everything a JOIN alongside it would touch.
fn vector_pass(
    index: &Index,
    qvec: &[f32],
    top_k: i64,
    acc: &mut BTreeMap<String, Acc>,
) -> Result<()> {
    // The query vector as a little-endian f32 blob — Turso reads it as a Float32-dense
    // vector directly (same layout the stored `vector` column uses). Zero/empty query
    // vectors were already filtered by `embed_query`.
    let qblob = vector::encode_vector(qvec);
    // `vector_distance_cos` hard-errors on a dimension mismatch, so only compare against
    // stored vectors of the same width (byte length ⇒ f32 count ⇒ dims). This makes a
    // stale-dimension index — one built before an embedding-dimensions change and not yet
    // `--re-embed`ed — degrade to "no vector recall" instead of failing the query.
    let qlen = qblob.len() as i64;

    let rows = index.query_rows(
        "SELECT node_id, section_line, vector_distance_cos(vector, ?1) AS dist
         FROM embeddings WHERE length(vector) = ?2
         ORDER BY dist ASC LIMIT ?3",
        turso::params![qblob, qlen, top_k],
        |r| Ok((col_text(r, 0)?, col_u32(r, 1)?, col_f64(r, 2)?)),
    )?;

    // Aggregate to each file's best (lowest-distance) section.
    let mut best: HashMap<String, (f32, u32)> = HashMap::new();
    for (id, line, dist) in rows {
        // A zero-magnitude stored vector (e.g. an empty section) gives an undefined
        // cosine, which Turso returns as NaN — skip it, matching the old brute-force
        // cosine, which returned 0 similarity for a zero vector.
        if !dist.is_finite() {
            continue;
        }
        let sim = 1.0 - dist as f32;
        best.entry(id)
            .and_modify(|(best_sim, best_line)| {
                if sim > *best_sim {
                    *best_sim = sim;
                    *best_line = line;
                }
            })
            .or_insert((sim, line));
    }

    // Node type/path for candidates neither `lexical_candidates` nor `name_pass` already
    // found.
    let missing: Vec<String> = best
        .keys()
        .filter(|id| !acc.contains_key(id.as_str()))
        .cloned()
        .collect();
    for (id, node_type, path) in nodes_info(index, &missing)? {
        acc.entry(id).or_insert_with(|| Acc::new(node_type, path));
    }
    for (id, (sim, line)) in best {
        if let Some(entry) = acc.get_mut(&id) {
            entry.vector_sim = sim;
            entry.vector_line = Some(line);
        }
    }
    Ok(())
}

/// Combine the three signals' own ranked lists into one score per file. Reciprocal-rank
/// fusion: `score = Σ w_s / (RRF_K + rank_s)` over whichever of lexical / name / vector
/// this file has a rank in at all (a file absent from a signal's list contributes `0.0`
/// for that term — never a rank of "last place"). Each signal's ranked list breaks ties by
/// `id` ascending, same as the final output (`BTreeMap` already iterates that way, so a
/// stable sort by score alone gets this for free).
///
/// The name signal is folded in twice, deliberately: every nonzero tier contributes its
/// usual weighted RRF term (so it can still tip a close race), and additionally, a tier at
/// or above [`NAME_GATE_TIER`] hard-outranks *every* file below that tier by adding the
/// tier number itself ahead of the (squashed-to-`[0, 1)`) RRF score — a name match this
/// precise should never lose to a merely-longer document, for any lexical/vector margin.
/// Gating every tier this way, including tier 1 ("shares one non-stopword token"),
/// regressed a few `document`/`keyword` queries — tier 1 alone stays a soft RRF
/// contributor, never a hard gate.
fn fuse(acc: &mut BTreeMap<String, Acc>) {
    if acc.is_empty() {
        return;
    }
    let lexical_rank = rank_signal(acc, |a| (a.lexical_score > 0.0).then_some(a.lexical_score));
    let name_rank = rank_signal(acc, |a| (a.name_tier > 0).then_some(a.name_tier as f32));
    let vector_rank = rank_signal(acc, |a| {
        (a.vector_sim > VECTOR_MIN_SIM).then_some(a.vector_sim)
    });

    for (id, entry) in acc.iter_mut() {
        let lex_term = rrf_term(lexical_rank.get(id), RRF_K);
        let name_term = RRF_W_NAME * rrf_term(name_rank.get(id), RRF_K);
        let vec_term = RRF_W_VEC * rrf_term(vector_rank.get(id), RRF_K);

        let gate = if entry.name_tier >= NAME_GATE_TIER {
            entry.name_tier as f32
        } else {
            0.0
        };
        entry.score = gate + squash_unit(lex_term + name_term + vec_term);
        entry.vector_leads = vec_term > lex_term;
    }
}

/// Order-preserving squash of a non-negative score into `[0, 1)`, so it can sit as the
/// fractional part behind an integer [`NAME_GATE_TIER`] gate in [`fuse`] without ever being
/// able to cross into the next tier up, for any input magnitude.
fn squash_unit(score: f32) -> f32 {
    if score > 0.0 {
        score / (score + 1.0)
    } else {
        0.0
    }
}

/// Rank every file `key` accepts, highest first, ties broken by `id` ascending (`acc`
/// already iterates that way, and the sort below is stable). A file `key` rejects
/// (returns `None`) has no rank — no presence in this signal at all.
fn rank_signal(
    acc: &BTreeMap<String, Acc>,
    key: impl Fn(&Acc) -> Option<f32>,
) -> HashMap<String, u32> {
    let mut items: Vec<(&String, f32)> = acc
        .iter()
        .filter_map(|(id, a)| key(a).map(|v| (id, v)))
        .collect();
    items.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    items
        .into_iter()
        .enumerate()
        .map(|(i, (id, _))| (id.clone(), (i + 1) as u32))
        .collect()
}

/// One reciprocal-rank-fusion term: `1 / (k + rank)`, or `0.0` with no rank in this signal.
fn rrf_term(rank: Option<&u32>, k: f32) -> f32 {
    rank.map(|&r| 1.0 / (k + r as f32)).unwrap_or(0.0)
}

/// Fill in the final anchors for the already-ranked, already-truncated hit list: the union
/// of a file's best lexical sections (by BM25 score, not line position) and its best
/// vector section, ordered by whichever signal contributed more to that file's fused score
/// (see `fuse`), capped at `MAX_ANCHORS`. A file with neither (a name-only match) falls
/// back to its first section, same as the pre-fusion baseline. Section bodies (needed only
/// for the snippet — `heading`/`line` already came from each pass's own scoring query) are
/// fetched here, in one batched call, for exactly the anchors of exactly these
/// already-truncated files — never the wider candidate set either pass matched.
fn resolve_anchors(index: &Index, tokens: &[String], selected: &mut [(String, Acc)]) -> Result<()> {
    let mut pairs: Vec<(String, u32)> = Vec::new();
    for (id, a) in selected.iter() {
        pairs.extend(
            a.lexical_sections
                .iter()
                .map(|(line, _)| (id.clone(), *line)),
        );
        pairs.extend(a.vector_line.map(|line| (id.clone(), line)));
    }
    let sections = anchor_sections(index, &pairs)?;

    let mut need_fallback: Vec<String> = Vec::new();

    for (id, a) in selected.iter_mut() {
        let lexical_anchors = a.lexical_sections.iter().map(|(line, heading)| {
            let body = sections
                .get(&(id.clone(), *line))
                .map(|(_, body)| body.as_str())
                .unwrap_or_default();
            Anchor {
                heading: heading.clone(),
                line: *line,
                snippet: snippet(body, tokens),
            }
        });

        let vector_anchor = a.vector_line.and_then(|line| {
            sections
                .get(&(id.clone(), line))
                .map(|(heading, body)| Anchor {
                    heading: heading.clone(),
                    line,
                    snippet: snippet(body, tokens),
                })
        });

        let mut ordered: Vec<Anchor> = Vec::with_capacity(MAX_ANCHORS + 1);
        if a.vector_leads {
            ordered.extend(vector_anchor);
            ordered.extend(lexical_anchors);
        } else {
            ordered.extend(lexical_anchors);
            ordered.extend(vector_anchor);
        }
        let mut seen_lines = HashSet::new();
        ordered.retain(|anchor| seen_lines.insert(anchor.line));
        ordered.truncate(MAX_ANCHORS);

        if ordered.is_empty() {
            need_fallback.push(id.clone());
        } else {
            a.anchors = ordered;
        }
    }

    if !need_fallback.is_empty() {
        let mut fallback: HashMap<String, Anchor> = first_sections(index, &need_fallback)?
            .into_iter()
            .map(|(id, heading, line, body)| {
                (
                    id,
                    Anchor {
                        heading,
                        line,
                        snippet: snippet(&body, tokens),
                    },
                )
            })
            .collect();
        for (id, a) in selected.iter_mut() {
            if a.anchors.is_empty()
                && let Some(anchor) = fallback.remove(id)
            {
                a.anchors.push(anchor);
            }
        }
    }
    Ok(())
}

/// `heading`/`body` for exactly the `(node_id, line)` pairs used as anchors among the
/// final hit list — never for the full candidate set either the lexical or vector pass
/// matched.
///
/// Narrows with `node_id IN (...) AND line IN (...)` — both plain column tests, so Turso
/// can use the `sections(node_id, line)` index — then keeps only the exact pairs actually
/// wanted (the two `IN` lists alone admit a cross product, e.g. node A's line the vector
/// pass wants and node B's line the lexical pass wants both being in each list without A's
/// line-that's-B's-anchor mattering). A *computed* key like `(node_id || sep || line) IN
/// (...)` cannot use that index at all and forces a full-table scan instead.
fn anchor_sections(
    index: &Index,
    pairs: &[(String, u32)],
) -> Result<HashMap<(String, u32), (String, String)>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let wanted: HashSet<(String, u32)> = pairs.iter().cloned().collect();
    let mut ids: Vec<String> = wanted.iter().map(|(id, _)| id.clone()).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut lines: Vec<u32> = wanted.iter().map(|(_, line)| *line).collect();
    lines.sort_unstable();
    lines.dedup();

    let (id_placeholders, id_params) = in_list_params(&ids, 1);
    let (line_placeholders, line_params) = in_list_params(&lines, ids.len() + 1);
    let mut params = id_params;
    params.extend(line_params);
    let sql = format!(
        "SELECT node_id, line, heading, body FROM sections
         WHERE node_id IN ({id_placeholders}) AND line IN ({line_placeholders})"
    );
    let rows = index.query_rows(&sql, params, |r| {
        Ok((
            col_text(r, 0)?,
            col_u32(r, 1)?,
            col_text(r, 2)?,
            col_text(r, 3)?,
        ))
    })?;
    Ok(rows
        .into_iter()
        .filter(|(id, line, ..)| wanted.contains(&(id.clone(), *line)))
        .map(|(id, line, heading, body)| ((id, line), (heading, body)))
        .collect())
}

/// `(type, path)` for node ids the vector pass recalled that neither `lexical_candidates`
/// nor `name_pass` had already found — one batched lookup instead of a JOIN in the distance
/// query itself.
fn nodes_info(index: &Index, ids: &[String]) -> Result<Vec<(String, String, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let (placeholders, params) = in_list_params(ids, 1);
    let sql = format!("SELECT id, type, path FROM nodes WHERE id IN ({placeholders})");
    index.query_rows(&sql, params, |r| {
        Ok((col_text(r, 0)?, col_text(r, 1)?, col_text(r, 2)?))
    })
}

/// Build a `(?1, ?2, …)` placeholder list, starting at `?{start}`, plus its bound params
/// for a `WHERE col IN (...)` clause — shared by every small batched lookup in this module.
fn in_list_params<T: Into<turso::Value> + Clone>(
    values: &[T],
    start: usize,
) -> (String, Vec<turso::Value>) {
    let placeholders = (start..start + values.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let params = values.iter().cloned().map(Into::into).collect();
    (placeholders, params)
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
                id: NodeId::parse_stored(&id),
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
        // Local edges only: a cross-package edge's bare to_id could coincide with the
        // container's id but points at another package's node.
        "SELECT from_id FROM edges WHERE to_package IS NULL AND ref_type = ?1 AND to_id = ?2",
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
    let (placeholders, params) = in_list_params(node_ids, 1);
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

/// Escape the LIKE metacharacters for use with `ESCAPE '\'`.
///
/// [`tokenize`] already yields alphanumeric-only tokens, so nothing reaches this today
/// that needs escaping — it is here so a future caller with a looser token source cannot
/// turn a `_` or `%` in a query into a silent wildcard.
fn like_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A short, whitespace-collapsed snippet windowed around the first *whole-token*
/// occurrence of a query token: a substring test would let "end" hit "endpoint" and other
/// unrelated words. Each candidate word is tokenized the same way the query itself is
/// ([`tokenize`]: lowercase, split on non-alphanumeric) and compared for an exact match.
fn snippet(body: &str, tokens: &[String]) -> String {
    let words: Vec<&str> = body.split_whitespace().collect();
    let hit = words
        .iter()
        .position(|w| tokenize(w).iter().any(|wt| tokens.contains(wt)));
    match hit {
        Some(i) => {
            let start = i.saturating_sub(4);
            let end = (i + 8).min(words.len());
            words[start..end].join(" ")
        }
        None => words.iter().take(12).copied().collect::<Vec<_>>().join(" "),
    }
}

// ---- workspace fan-out (M5, design.md §9) ----------------------------------

use std::path::PathBuf;
use std::rc::Rc;

use crate::workspace::{PackageHandle, Workspace, resolver};

/// A search hit located in a workspace member. `hit.id` is qualified (`@pkg/…`) exactly
/// when the member is not the run-root package.
pub struct WsHit {
    pub package: Option<String>,
    /// The member's canonical root (for consumer-relative display paths).
    pub root: PathBuf,
    pub hit: SearchHit,
}

/// A suggestion located in a workspace member (same conventions as [`WsHit`]).
pub struct WsSuggestion {
    pub package: Option<String>,
    pub root: PathBuf,
    pub suggestion: Suggestion,
}

/// Which members a fan-out read consults, plus the dependencies it could not
/// (unlinked / broken / no index) — surfaced, never silently dropped.
///
/// Routing: an `@pkg/…` `--scope` selects exactly that member; `--local` restricts to
/// the run-root; otherwise the run-root + its whole locatable closure.
fn members_for(
    ws: &Workspace,
    scope_pkg: Option<&str>,
    local: bool,
) -> Result<(Vec<Rc<PackageHandle>>, Vec<String>)> {
    let current = ws.current();
    if let Some(alias) = scope_pkg {
        let member = resolver::step_into(ws, &current, alias)?;
        return Ok((vec![member], Vec::new()));
    }
    if local {
        return Ok((vec![current], Vec::new()));
    }
    let (members, skipped) = ws.consult_closure();
    Ok((members, skipped))
}

/// [`search`] across the run-root + its dependency closure: the query is embedded ONCE,
/// each member runs the same three passes *and the same [`fuse`]* against its own index.
/// Fusion is reciprocal-rank (plus the name tier's hard gate), so a file's score depends on
/// its rank within that member's own candidate set and its own name-tier gate, not on any
/// shared scale — "comparable" now means identical ranks (and identical gates) score
/// identically everywhere (same code, same constants), not that two members' top hits
/// reflect equally strong matches. A weak best-in-member match can still outrank a stronger
/// one from a deeper member. Per-member results carry the per-member LIMIT, and the merge
/// re-ranks by (score desc, qualified id asc) before the global limit; fusing across the
/// whole merged pool instead of per member would fix this, but needs each member to expose
/// its raw per-signal candidates before ranking — a bigger change than this pass, not
/// implemented here. A member whose index is unavailable is skipped and surfaced — except
/// the run-root, whose failure is the classic local error.
pub fn search_workspace(
    ws: &Workspace,
    embedder: &dyn Embedder,
    query: &str,
    opts: &SearchOpts,
    local: bool,
) -> Result<(Vec<WsHit>, Vec<String>)> {
    let scope_pkg = opts
        .scope
        .as_ref()
        .and_then(|s| s.package())
        .map(str::to_string);
    let (members, mut skipped) = members_for(ws, scope_pkg.as_deref(), local)?;
    let current_root = ws.current().root.clone();

    let qvec = embed_query(embedder, query)?;
    // The member-local view of --scope: the container id without its @pkg/ qualifier
    // (scope edges store bare within-package targets).
    let member_opts = SearchOpts {
        type_filter: opts.type_filter.clone(),
        scope: opts.scope.as_ref().map(|s| {
            let mut bare = s.clone();
            bare.package = None;
            bare
        }),
        limit: opts.limit,
        scope_field: opts.scope_field.clone(),
    };

    let mut all: Vec<WsHit> = Vec::new();
    for member in members {
        let is_run_root = member.root == current_root;
        let index = match member.index() {
            Ok(index) => index,
            Err(e) if is_run_root => return Err(e),
            Err(e) if scope_pkg.is_some() => return Err(e), // the one member asked for
            Err(_) => {
                skipped.push(member.id.to_string());
                continue;
            }
        };
        for mut hit in search_prepared(index, qvec.as_deref(), query, &member_opts)? {
            if !is_run_root {
                hit.id = hit.id.with_package(member.id.0.clone());
            }
            all.push(WsHit {
                package: (!is_run_root).then(|| member.id.to_string()),
                root: member.root.clone(),
                hit,
            });
        }
    }

    all.sort_by(|x, y| {
        y.hit
            .score
            .partial_cmp(&x.hit.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.hit.id.cmp(&y.hit.id))
    });
    all.truncate(opts.limit.unwrap_or(10));
    skipped.sort();
    skipped.dedup();
    Ok((all, skipped))
}

/// [`suggest`] across the run-root + its dependency closure (same member routing and
/// merge discipline as [`search_workspace`]; no vectors, so nothing to share).
pub fn suggest_workspace(
    ws: &Workspace,
    descriptor: &str,
    type_filter: Option<&NodeType>,
    limit: usize,
    local: bool,
) -> Result<(Vec<WsSuggestion>, Vec<String>)> {
    let (members, mut skipped) = members_for(ws, None, local)?;
    let current_root = ws.current().root.clone();

    let mut all: Vec<WsSuggestion> = Vec::new();
    for member in members {
        let is_run_root = member.root == current_root;
        let index = match member.index() {
            Ok(index) => index,
            Err(e) if is_run_root => return Err(e),
            Err(_) => {
                skipped.push(member.id.to_string());
                continue;
            }
        };
        for mut suggestion in suggest(index, descriptor, type_filter, limit)? {
            if !is_run_root {
                suggestion.id = suggestion.id.with_package(member.id.0.clone());
            }
            all.push(WsSuggestion {
                package: (!is_run_root).then(|| member.id.to_string()),
                root: member.root.clone(),
                suggestion,
            });
        }
    }

    all.sort_by(|a, b| {
        b.suggestion
            .score
            .partial_cmp(&a.suggestion.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.suggestion.id.cmp(&b.suggestion.id))
    });
    all.truncate(limit);
    skipped.sort();
    skipped.dedup();
    Ok((all, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::db::Index;

    /// Proves the production ranked statement in [`fts_pass_candidates`] gets real,
    /// non-zero, varying BM25 scores from Turso/Tantivy — not the silent all-`0.0`
    /// fallback the `fts_score` hazard would otherwise leave undetected. A repeated,
    /// heading-boosted term ranks above a single mention, which in turn ranks above no
    /// mention at all.
    #[test]
    fn fts_score_returns_real_varying_scores() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index =
            Index::create_for_bulk_load(&dir.path().join("index.db")).expect("create index");
        index
            .execute(
                "INSERT INTO sections (node_id, heading, line, body) VALUES (?1, ?2, ?3, ?4)",
                turso::params![
                    "doc-a",
                    "Ingest pipeline",
                    1i64,
                    "The ingest pipeline handles ingest volume for the service."
                ],
            )
            .expect("insert doc-a");
        index
            .execute(
                "INSERT INTO sections (node_id, heading, line, body) VALUES (?1, ?2, ?3, ?4)",
                turso::params![
                    "doc-b",
                    "Overview",
                    1i64,
                    "This section briefly mentions ingest once in passing."
                ],
            )
            .expect("insert doc-b");
        index
            .execute(
                "INSERT INTO sections (node_id, heading, line, body) VALUES (?1, ?2, ?3, ?4)",
                turso::params![
                    "doc-c",
                    "Unrelated",
                    1i64,
                    "Nothing to do with the query terms at all."
                ],
            )
            .expect("insert doc-c");
        index.ensure_fts_index().expect("build fts index");

        let rows = fts_pass_candidates(&index, "ingest", Some(10)).expect("query candidates");

        assert_eq!(
            rows.len(),
            2,
            "doc-c has no match and must not be returned: {rows:?}"
        );
        assert!(
            rows.iter().all(|(_, _, _, score)| *score > 0.0),
            "every matched row must carry a real, non-zero BM25 score: {rows:?}"
        );
        assert_ne!(
            rows[0].3, rows[1].3,
            "scores must vary by relevance, not be a flat fallback value: {rows:?}"
        );
        assert_eq!(
            rows[0].0, "doc-a",
            "the heading + repeated-body match must outrank the single mention: {rows:?}"
        );
    }

    #[test]
    fn snippet_matches_whole_tokens_only() {
        let tokens = vec!["end".to_string()];
        // "endpoint" contains "end" as a substring but must not match.
        assert_eq!(
            snippet("the endpoint is slow", &tokens),
            "the endpoint is slow"
        );
        let tokens = vec!["throughput".to_string()];
        assert!(snippet("we raised throughput concerns today", &tokens).contains("throughput"));
    }

    #[test]
    fn name_match_tier_grades_exact_subset_and_partial() {
        let exact = tokenize("loose end");
        assert_eq!(name_match_tier(&tokenize("loose end"), &exact), 4);
        assert_eq!(
            name_match_tier(&tokenize("logistics"), &tokenize("logistics contact")),
            3
        );
        assert_eq!(
            name_match_tier(&tokenize("what is a loose end"), &tokenize("loose end")),
            2
        );
        assert_eq!(
            name_match_tier(&tokenize("loose thread"), &tokenize("loose end")),
            1
        );
        assert_eq!(name_match_tier(&tokenize("zzz"), &tokenize("loose end")), 0);
    }
}
