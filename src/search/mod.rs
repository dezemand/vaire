//! Hybrid search: lexical + name/alias + vector signals, combined by reciprocal-rank fusion
//! (design.md §9, cli.md §3.4). The **file is the returned unit**, with its best-matching
//! sections as anchors. Results sort by descending score; ties break by `id` ascending.
//!
//! Three signals feed the ranking, each scored by its own pass:
//! - **lexical** ([`lexical_candidates`]) — the query ([`build_match_query`]) drops
//!   stopwords and adds inflection variants at a down-weighted boost (the index tokenizer
//!   does no stemming); Tantivy's `fts_score` triages candidate sections cheaply, then
//!   [`section_bm25`] re-scores them in Rust (corpus IDF, a heading boost, section-length
//!   normalisation, whole-token frequency), and [`aggregate_sections`] reduces a file's
//!   matching sections to one score.
//! - **name** ([`name_pass`]) — graded name/alias/id-slug tiers (design.md §8), the same
//!   precision `suggest` already has.
//! - **vector** ([`vector_pass`]) — brute-force cosine top-K, a noise floor rather than a
//!   precision gate (a fixed high-confidence threshold routinely returns *zero* sections
//!   for a real embedder, so a floor is applied only once signals are combined).
//!
//! [`fuse`] combines the three into the score results are ordered by: reciprocal-rank
//! fusion over all three, plus a hard gate — an exact name/alias/id-slug match outranks
//! every file without one, regardless of lexical/vector standing (see [`fuse`] for exactly
//! where that line sits and why).
//!
//! (Reference resolution, design.md §8, is the *same* machinery used in the other
//! direction — alias + FTS first, embeddings as backup — and will live alongside this.)

mod inflect;
mod text;
pub mod vector;

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::embed::Embedder;
use crate::error::Result;
use crate::index::db::{Index, col_f64, col_i64, col_text, col_u32};
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
// Tuned on the search benchmark (issue #52); each comment below explains the trade-off a
// constant sits on, not the tuning process.

/// How many candidate sections `fts_match`/`fts_score` considers, ranked by score, before
/// per-file MAX aggregation. Bounding this bounds the follow-up [`node_meta`] lookup to a
/// fixed-size candidate set instead of however many sections in the whole corpus matched:
/// quality-identical to a much larger/unlimited cap, but meaningfully faster on a large
/// corpus.
const FTS_CANDIDATE_CAP: Option<i64> = Some(100);

/// How many sections the vector pass pulls, ranked by raw cosine distance. No dense-vector
/// index exists, so this stays a brute-force `ORDER BY dist LIMIT`; this bounds that scan.
const VECTOR_TOP_K: i64 = 20;
/// Minimum cosine similarity for a file to count as vector-recalled at all. Far below a
/// fixed high-confidence gate, but not zero: with no floor at all, [`VECTOR_TOP_K`]
/// sections are pulled for *every* query regardless of relevance (a brute-force top-K has
/// no "no match" outcome), which would make even a nonsense query surface filler results.
/// This is a noise floor, not a precision gate — precision still comes from lexical +
/// name.
const VECTOR_MIN_SIM: f32 = 0.3;

/// Reciprocal-rank fusion (see [`fuse`]): `score = Σ w_s / (RRF_K + rank_s)` over each
/// signal a file appears in. RRF's rank-damping constant: a small value lets 1st place
/// lead more decisively, which suits how clean each signal's own ranking already is here
/// (each is either a real match or absent, not a noisy retriever).
const RRF_K: f32 = 5.0;
/// Vector's RRF weight, at parity with lexical (whose own term is unweighted, `1.0`).
const RRF_W_VEC: f32 = 1.0;
/// The name signal's RRF weight — folded in alongside lexical/vector *before* the
/// [`NAME_GATE_TIER`] hard gate is applied (see [`fuse`]), so a name match below the gate
/// still nudges the ranking instead of contributing nothing. Kept low: even a weak
/// (tier 1) partial-token match must not crowd out a genuinely better lexical/vector
/// candidate in the RRF sum.
const RRF_W_NAME: f32 = 1.0;
/// The name/alias/id-slug tier (see [`name_match_tier`]) at or above which a match hard-
/// outranks every file below that tier, regardless of lexical/vector standing. Set to
/// exact matches only (tier 4): gating a looser tier (e.g. tier 3, "every query token
/// present in the candidate" — a short name inside a long question) hard-outranked some
/// genuinely better `document`/`question` matches and let documents intrude at rank 1
/// more often. Tiers 1-3 still feed the fused score via [`RRF_W_NAME`], just without the
/// hard gate.
const NAME_GATE_TIER: u8 = 4;

// BM25F-lite constants for `section_bm25`, the score that actually orders lexical
// results. `BM25_K1`/`BM25_B` mirror Tantivy's own fixed BM25 constants, even though this
// score is computed independently in Rust: `fts_candidates` also calls Turso's native
// `fts_score`, but only to triage which candidates are worth a body fetch — the ranked
// statement cannot JOIN `nodes` or aggregate per file, and `fts_score`'s own field weights
// apply before this module's heading boost, so `section_bm25` is what actually orders
// results. `W_HEADING` implements that heading boost: a heading token — or, for the
// preamble, a token from its `# Title` line (see [`text::split_title_line`]) — counts
// double before the saturation curve, so a heading/title match is no longer scored the
// same as an ordinary body word.
const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;
const W_HEADING: f32 = 2.0;

/// Common English stopwords, dropped from the lexical query and from [`section_bm25`]'s
/// re-score when at least one non-stopword token remains. With a stopword left in, a long
/// document that merely contains "the" or "a" many times can occupy candidate slots (and
/// re-score points) a more topically relevant short section would otherwise take.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "if", "of", "in", "on", "at", "to", "for", "with", "by",
    "from", "as", "is", "are", "was", "were", "be", "been", "being", "this", "that", "these",
    "those", "it", "its", "we", "you", "your", "our", "i", "do", "does", "did", "have", "has",
    "had", "not", "no", "so", "than", "then", "there", "their", "them", "he", "she", "his", "her",
    "will", "would", "can", "could", "should", "about", "into", "up", "down", "out", "over",
    "under", "again", "what", "which", "who", "whom", "how", "when", "where", "why",
];

/// Weight applied to a token's inflection variants, both as a Tantivy `term^boost` in the
/// lexical query and as [`ScoredTerm::weight`] in the Rust re-score: real tokens compete
/// at full strength, a plausible variant (e.g. "entities" for query token "entity") only
/// recovers a match the index's non-stemming tokenizer would otherwise miss entirely,
/// without letting a wrong guess outrank an exact hit.
const INFLECTION_BOOST: f32 = 0.5;

/// Drop [`STOPWORDS`] from `tokens`, unless doing so would leave nothing (an all-stopword
/// query must still search on *something*).
fn drop_stopwords(tokens: &[String]) -> Vec<String> {
    let filtered: Vec<String> = tokens
        .iter()
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .cloned()
        .collect();
    if filtered.is_empty() {
        tokens.to_vec()
    } else {
        filtered
    }
}

/// Expand `tokens` with each token's [`inflect::variants`], at `boost` (Tantivy
/// `term^boost` syntax). A variant already present among the real tokens (or a duplicate
/// across two tokens' variant sets) is skipped rather than emitted twice: Tantivy's parser
/// turns repeated terms into separate clauses that would silently stack a term's effective
/// weight past what `boost` intends.
fn add_inflection_variants(tokens: &[String], boost: f32) -> Vec<String> {
    let mut seen: HashSet<String> = tokens.iter().cloned().collect();
    let mut parts: Vec<String> = tokens.to_vec();
    for t in tokens {
        for v in inflect::variants(t) {
            if seen.insert(v.clone()) {
                parts.push(format!("{v}^{boost}"));
            }
        }
    }
    parts
}

/// Build the lexical match query bound to both `fts_match` and `fts_score` (the same `?1`
/// must reach both): stopwords dropped ([`drop_stopwords`]), then each remaining token's
/// inflection variants added at [`INFLECTION_BOOST`] ([`add_inflection_variants`]).
fn build_match_query(tokens: &[String]) -> String {
    add_inflection_variants(&drop_stopwords(tokens), INFLECTION_BOOST).join(" ")
}

/// Words a matched snippet should highlight around: `tokens` plus their plain inflection
/// variants (no Tantivy boost suffix — [`snippet`] compares whole words, not query
/// syntax). A section whose only lexical hit is "entities" for query token "entity"
/// should still get a snippet centred on that occurrence, not the first-12-words
/// fallback.
fn snippet_words(tokens: &[String]) -> Vec<String> {
    let mut words = tokens.to_vec();
    for t in tokens {
        for v in inflect::variants(t) {
            if !words.contains(&v) {
                words.push(v);
            }
        }
    }
    words
}

/// One literal token counted during the Rust BM25 re-score ([`section_bm25`]): either a
/// real (non-stopword) query token at full weight, or one of its inflection variants at
/// [`INFLECTION_BOOST`] — the same down-weighting [`build_match_query`] gives that variant
/// in the Tantivy candidate query, applied again here since `fts_score` never becomes the
/// final ranking (see the module doc comment).
struct ScoredTerm {
    text: String,
    weight: f32,
}

/// The terms [`section_bm25`] scores a section against: [`STOPWORDS`] dropped the same way
/// as [`build_match_query`] (a dropped stopword is not scored at all), plus each surviving
/// token's inflection variants, each counted separately with its own corpus IDF — a
/// variant is a different literal token from its base form, so it has its own document
/// frequency.
fn scored_terms(tokens: &[String]) -> Vec<ScoredTerm> {
    let base = drop_stopwords(tokens);
    let mut seen: HashSet<String> = base.iter().cloned().collect();
    let mut terms: Vec<ScoredTerm> = base
        .iter()
        .map(|t| ScoredTerm {
            text: t.clone(),
            weight: 1.0,
        })
        .collect();
    for t in &base {
        for v in inflect::variants(t) {
            if seen.insert(v.clone()) {
                terms.push(ScoredTerm {
                    text: v,
                    weight: INFLECTION_BOOST,
                });
            }
        }
    }
    terms
}

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

/// The lexical signal: Tantivy's own `fts_score` triages candidate sections cheaply (its
/// field weights apply before this module's heading boost and it can't aggregate per
/// file, so it never becomes the final ranking — only which sections are worth a body
/// fetch and a proper re-score), then [`section_bm25`] scores each candidate in Rust with
/// corpus IDF, section-length normalisation, a heading boost, and whole-token frequency
/// instead of substring counts. A file's matching sections are combined into one per-file
/// score by [`aggregate_sections`] (MAX): a long document no longer wins just by having
/// more matching sections. Everything downstream ([`fuse`], `resolve_anchors`) only ever
/// reads the resulting [`LexicalMatch`], never the underlying sections.
///
/// The ranked candidate statement ([`fts_candidates`]) is one of the two patterns Turso's
/// ranked optimizer path actually matches: no JOIN, the same `?1` bound to both
/// `fts_match` and `fts_score`, and — when capped, as here — a plain integer `LIMIT`. A
/// JOIN or an extra `ORDER BY` key silently drops to the *fallback* pattern, which returns
/// `0.0` for every row with no error, so `node_type`/`path` are fetched by a separate query
/// ([`node_meta`]) instead of being joined into this one.
fn lexical_candidates(index: &Index, tokens: &[String]) -> Result<BTreeMap<String, LexicalMatch>> {
    let match_query = build_match_query(tokens);
    let mut candidates = fts_candidates(index, &match_query, FTS_CANDIDATE_CAP)?;
    if !candidates.is_empty() && candidates.iter().all(|c| c.tantivy_score == 0.0) {
        // Every fts_match'd row scored exactly 0.0: the ranked statement's optimizer
        // pattern did not match and Turso silently fell back to its ranking-blind path. A
        // capped LIMIT chosen from that untrustworthy ORDER BY could silently drop real
        // matches, so fall back to every fts_match'd candidate uncapped rather than serve
        // results silently missing whatever the broken ranking pushed out. Loud in
        // debug/test builds; degrades gracefully in release rather than serve unranked (or
        // wrongly capped) results with no indication anything is wrong.
        debug_assert!(
            false,
            "fts_score returned 0.0 for every fts_match row — the ranked Tantivy query \
             pattern likely stopped matching; falling back to the uncapped candidate set"
        );
        candidates = fts_candidates(index, &match_query, None)?;
    }
    if candidates.is_empty() {
        return Ok(BTreeMap::new());
    }

    // Corpus statistics for this query: total section count, the average section length
    // (a character proxy, computed the same way per section in `section_bm25`), and each
    // scored term's document frequency for the classic BM25 IDF — including inflection
    // variants, each a distinct literal token with its own document frequency. N and
    // avgdl in one round trip (both are whole-corpus aggregates, independent of the query
    // and of the candidate cap above) rather than two, and every term's df likewise in one
    // round trip: a `SELECT` of N scalar subqueries, each the exact single-token
    // `fts_match` shape already known to hit the FTS index, rather than N separate
    // statements.
    let (n, avgdl) = index
        .query_opt(
            "SELECT count(*), avg(length(heading) + length(body)) FROM sections",
            (),
            |r| Ok((col_i64(r, 0)? as f32, col_f64(r, 1)?.max(1.0) as f32)),
        )?
        .unwrap_or((0.0, 1.0));
    let terms = scored_terms(tokens);
    let df_sql = format!(
        "SELECT {}",
        (1..=terms.len())
            .map(|i| format!(
                "(SELECT count(*) FROM sections WHERE fts_match(heading, body, ?{i}))"
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let df_params: Vec<turso::Value> = terms
        .iter()
        .map(|t| turso::Value::from(t.text.clone()))
        .collect();
    let dfs: Vec<i64> = index
        .query_opt(&df_sql, df_params, |r| {
            (0..terms.len()).map(|i| col_i64(r, i)).collect()
        })?
        .unwrap_or_else(|| vec![0; terms.len()]);
    let mut idf: HashMap<String, f32> = HashMap::with_capacity(terms.len());
    for (t, df) in terms.iter().zip(dfs) {
        let df = df as f32;
        idf.insert(t.text.clone(), (1.0 + (n - df + 0.5) / (df + 0.5)).ln());
    }

    // Bodies for exactly the surviving candidates, fetched by primary key — never for
    // every fts_match'd section, however many a long document contributes.
    let rowids: Vec<i64> = candidates.iter().map(|c| c.rowid).collect();
    let mut bodies = candidate_bodies(index, &rowids)?;

    // Group surviving candidates by file, pairing each with its body (`remove` rather
    // than `get().clone()`: each rowid is a distinct physical row, so it appears at most
    // once across `candidates` and its body is never needed a second time).
    let mut by_node: HashMap<String, Vec<(String, u32, String)>> = HashMap::new();
    for c in candidates {
        let Some(body) = bodies.remove(&c.rowid) else {
            continue; // defensive: rowid vanished between the two queries (shouldn't happen on a read-only index)
        };
        by_node
            .entry(c.node_id)
            .or_default()
            .push((c.heading, c.line, body));
    }
    if by_node.is_empty() {
        return Ok(BTreeMap::new());
    }
    let ids: Vec<String> = by_node.keys().cloned().collect();
    let meta = node_meta(index, &ids)?;

    let mut out: BTreeMap<String, LexicalMatch> = BTreeMap::new();
    for (id, sections) in by_node {
        let Some((node_type, path)) = meta.get(&id) else {
            continue; // defensive: a matched section with no owning node (should not happen)
        };
        let mut scored: Vec<(f32, u32, String)> = sections
            .into_iter()
            .filter_map(|(heading, line, body)| {
                let score = section_bm25(&heading, &body, &terms, &idf, avgdl);
                (score > 0.0).then_some((score, line, heading))
            })
            .collect();
        if scored.is_empty() {
            continue;
        }
        // Best sections first (score desc, ties by line asc) — anchor selection reads off
        // this same order, keeping the best-matching sections rather than whichever
        // happened to come first by line position.
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
        let section_scores: Vec<f32> = scored.iter().map(|(s, ..)| *s).collect();
        let sections: Vec<(u32, String)> = scored
            .into_iter()
            .take(MAX_ANCHORS)
            .map(|(_, line, heading)| (line, heading))
            .collect();
        out.insert(
            id,
            LexicalMatch {
                node_type: node_type.clone(),
                path: path.clone(),
                score: aggregate_sections(&section_scores),
                sections,
            },
        );
    }
    Ok(out)
}

/// One ranked FTS candidate section — [`fts_candidates`]'s row shape. `tantivy_score` is
/// used only to decide which candidates survive [`FTS_CANDIDATE_CAP`] and to guard against
/// the silent-fallback hazard; the score that actually ranks results is [`section_bm25`],
/// computed later from the fetched `body`.
#[derive(Debug)]
struct Candidate {
    rowid: i64,
    node_id: String,
    heading: String,
    line: u32,
    tantivy_score: f32,
}

/// The ranked candidate statement: no JOIN, the same `?1` bound to both `fts_match` and
/// `fts_score`, a plain integer `LIMIT` when capped. `cap = None` uses the other verified
/// shape (no `ORDER BY`/`LIMIT` at all — used as this module's uncapped fallback).
/// `rowid` rides along for free: the optimizer's structural match on this statement
/// compares FROM/WHERE/ORDER BY/LIMIT, never the projection list.
fn fts_candidates(index: &Index, match_query: &str, cap: Option<i64>) -> Result<Vec<Candidate>> {
    match cap {
        Some(limit) => index.query_rows(
            "SELECT rowid, node_id, heading, line, fts_score(heading, body, ?1) AS score
             FROM sections WHERE fts_match(heading, body, ?1)
             ORDER BY score DESC LIMIT ?2",
            turso::params![match_query, limit],
            candidate_row,
        ),
        None => index.query_rows(
            "SELECT rowid, node_id, heading, line, fts_score(heading, body, ?1) AS score
             FROM sections WHERE fts_match(heading, body, ?1)",
            [match_query],
            candidate_row,
        ),
    }
}

/// Row mapper shared by both [`fts_candidates`] statement shapes.
fn candidate_row(r: &turso::Row) -> Result<Candidate> {
    Ok(Candidate {
        rowid: col_i64(r, 0)?,
        node_id: col_text(r, 1)?,
        heading: col_text(r, 2)?,
        line: col_u32(r, 3)?,
        tantivy_score: col_f64(r, 4)? as f32,
    })
}

/// `body` for exactly the candidate rows [`lexical_candidates`] decided to re-score,
/// fetched by primary key rather than by the `(node_id, line)` pair the rest of this
/// module otherwise keys on — `sections` is an ordinary rowid table, so the `rowid` the
/// ranked statement above projects addresses the same row here (confirmed by
/// `fts_candidates_returns_real_scores_and_correct_rowids`). A flat `IN (...)` rather than
/// a `rowid = ? OR ...` chain: enough distinct matched rows can otherwise hit Turso's
/// "Expression tree is too large" parser guard.
fn candidate_bodies(index: &Index, rowids: &[i64]) -> Result<HashMap<i64, String>> {
    if rowids.is_empty() {
        return Ok(HashMap::new());
    }
    let (placeholders, params) = in_list_params(rowids, 1);
    let sql = format!("SELECT rowid, body FROM sections WHERE rowid IN ({placeholders})");
    let rows = index.query_rows(&sql, params, |r| Ok((col_i64(r, 0)?, col_text(r, 1)?)))?;
    Ok(rows.into_iter().collect())
}

/// Combine one file's matching-section BM25 scores into its per-file score, given already
/// sorted descending by the caller (anchor selection reads off the same order): the single
/// best-matching section, rather than a sum, decay, or blend over several. A long file no
/// longer earns score merely for having more matching sections.
fn aggregate_sections(scores: &[f32]) -> f32 {
    scores.first().copied().unwrap_or(0.0)
}

/// BM25F-lite score for one section against `terms` (each already carrying its own
/// `weight` — see [`scored_terms`]): combined term frequency `tf_body + W_HEADING *
/// tf_heading`, corpus `idf` (from `lexical_candidates`), and length normalisation against
/// `avgdl` using the section's own character length — the exact character proxy `avgdl`
/// was computed with, so the two stay comparable. A term's full BM25 contribution (its own
/// idf and tf-saturation curve) is scaled by its `weight`, mirroring how the Tantivy
/// candidate query's `term^boost` syntax scales a boosted term's contribution rather than
/// its raw frequency.
fn section_bm25(
    heading: &str,
    body: &str,
    terms: &[ScoredTerm],
    idf: &HashMap<String, f32>,
    avgdl: f32,
) -> f32 {
    // The preamble's `# Title` line lives in `body` (its `heading` column is empty); pull
    // it out and weight it like a real heading so a document's title counts for more than
    // ordinary prose.
    let (title, body_rest) = if heading.is_empty() {
        text::split_title_line(body)
    } else {
        (None, body)
    };
    let body_counts = text::term_counts(body_rest);
    let heading_counts = match title {
        Some(t) => text::term_counts(t),
        None => text::term_counts(heading),
    };
    // Length normalisation uses the section's real stored size, not the title-split text,
    // so it stays the exact character proxy `avgdl` was computed with.
    let l = (heading.chars().count() + body.chars().count()) as f32;
    let norm = 1.0 - BM25_B + BM25_B * (l / avgdl);

    let mut score = 0.0f32;
    for term in terms {
        let tf = body_counts.get(&term.text).copied().unwrap_or(0) as f32
            + W_HEADING * heading_counts.get(&term.text).copied().unwrap_or(0) as f32;
        if tf <= 0.0 {
            continue;
        }
        let idf_t = idf.get(&term.text).copied().unwrap_or(0.0);
        score += term.weight * idf_t * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * norm);
    }
    score
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

/// A small English stopword list for the name-match signal only (design.md §8/§9) — kept
/// separate from [`STOPWORDS`] (measured on the search benchmark: sharing one list
/// regressed public MRR/nDCG on both embedders, since a few of the extra function words
/// [`STOPWORDS`] drops from the lexical query — e.g. "not", "he" — also occur, meaningfully,
/// inside real node names/aliases). Function words shouldn't count as meaningful overlap
/// when a query is graded against a node's name/alias/id-slug candidates: without this, a
/// query like "what is a loose end" would share a "token" with almost every node.
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
/// `suggest`'s own exact/token-subset match (not changed here) is a coarser 2-tier version
/// of the same idea, with no id-slug candidate — the same grading would improve it too.
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

/// Vector recall: brute-force top-K nearest sections by cosine distance — top-K sections;
/// a similarity floor ([`VECTOR_MIN_SIM`]) is applied only once signals are combined, not
/// here (an unconditional high-confidence gate at this stage would mean a real embedder
/// essentially never contributes). No JOIN: type/path for genuinely new candidates and
/// heading/body for anchors are fetched separately (`nodes_info`, `resolve_anchors`), so
/// this scan never pulls prose or node metadata for the ~everything a JOIN alongside it
/// would touch.
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
/// tier number itself ahead of the (squashed-to-`[0, 1)`) RRF score — an exact name/alias
/// match should never lose to a merely-longer document, for any lexical/vector margin.
/// Gating a looser tier this way (e.g. "every query token present in the candidate") let
/// more documents intrude at rank 1 without helping precision elsewhere — tiers below
/// [`NAME_GATE_TIER`] stay soft RRF contributors, never a hard gate.
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
    // A section whose only lexical hit is an inflection variant (e.g. "entities" for
    // query token "entity") should still get its snippet centred on that occurrence.
    let snippet_words = snippet_words(tokens);

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
                snippet: snippet(body, &snippet_words),
            }
        });

        let vector_anchor = a.vector_line.and_then(|line| {
            sections
                .get(&(id.clone(), line))
                .map(|(heading, body)| Anchor {
                    heading: heading.clone(),
                    line,
                    snippet: snippet(body, &snippet_words),
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
                        snippet: snippet(&body, &snippet_words),
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

    /// Proves the production ranked statement in [`fts_candidates`] gets real, non-zero,
    /// varying BM25 scores from Turso/Tantivy — not the silent all-`0.0` fallback the
    /// `fts_score` hazard would otherwise leave undetected — and that its `rowid`
    /// addresses the same row a direct lookup does, which [`candidate_bodies`] depends on.
    #[test]
    fn fts_candidates_returns_real_scores_and_correct_rowids() {
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

        let candidates = fts_candidates(&index, "ingest", Some(10)).expect("query candidates");

        assert_eq!(
            candidates.len(),
            2,
            "doc-c has no match and must not be returned: {candidates:?}"
        );
        assert!(
            candidates.iter().all(|c| c.tantivy_score > 0.0),
            "every matched row must carry a real, non-zero BM25 score: {candidates:?}"
        );
        assert_ne!(
            candidates[0].tantivy_score, candidates[1].tantivy_score,
            "scores must vary by relevance, not be a flat fallback value: {candidates:?}"
        );
        assert_eq!(
            candidates[0].node_id, "doc-a",
            "the heading + repeated-body match must outrank the single mention: {candidates:?}"
        );

        // rowid must address the same row a direct lookup does — candidate_bodies depends
        // on this.
        let rowids: Vec<i64> = candidates.iter().map(|c| c.rowid).collect();
        let bodies = candidate_bodies(&index, &rowids).expect("fetch bodies by rowid");
        for c in &candidates {
            let body = bodies
                .get(&c.rowid)
                .unwrap_or_else(|| panic!("body present for candidate rowid {}", c.rowid));
            assert!(
                body.contains("ingest"),
                "rowid {} must address {}'s own row, not some other section: {body:?}",
                c.rowid,
                c.node_id
            );
        }
    }

    /// The uncapped shape (no `ORDER BY`/`LIMIT`) is the other verified `fts_score`
    /// pattern — [`lexical_candidates`]'s release fallback if the capped shape ever
    /// returns all-zero scores. Proves it also returns real, non-zero scores, and every
    /// match (not just a capped top-N).
    #[test]
    fn fts_candidates_uncapped_returns_every_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index =
            Index::create_for_bulk_load(&dir.path().join("index.db")).expect("create index");
        for i in 0..5 {
            index
                .execute(
                    "INSERT INTO sections (node_id, heading, line, body) VALUES (?1, ?2, ?3, ?4)",
                    turso::params![format!("doc-{i}"), "", i as i64, "ingest pipeline volume"],
                )
                .expect("insert section");
        }
        index.ensure_fts_index().expect("build fts index");

        let candidates = fts_candidates(&index, "ingest", None).expect("query candidates");
        assert_eq!(
            candidates.len(),
            5,
            "uncapped must return every match: {candidates:?}"
        );
        assert!(candidates.iter().all(|c| c.tantivy_score > 0.0));
    }

    /// A capped query must return at most the cap, ranked so the cap keeps the
    /// highest-`fts_score` candidates first — what makes truncating to the cap safe.
    #[test]
    fn fts_candidates_cap_keeps_top_scoring_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index =
            Index::create_for_bulk_load(&dir.path().join("index.db")).expect("create index");
        for i in 0..20 {
            // Every 5th section repeats "ingest" many times, so it must outscore the rest.
            let body = if i % 5 == 0 {
                "ingest ingest ingest ingest ingest".to_string()
            } else {
                "ingest appears once here".to_string()
            };
            index
                .execute(
                    "INSERT INTO sections (node_id, heading, line, body) VALUES (?1, ?2, ?3, ?4)",
                    turso::params![format!("doc-{i}"), "", i as i64, body],
                )
                .expect("insert section");
        }
        index.ensure_fts_index().expect("build fts index");

        let capped = fts_candidates(&index, "ingest", Some(4)).expect("query candidates");
        assert_eq!(capped.len(), 4, "must respect the cap: {capped:?}");
        let heavy_hitters = capped
            .iter()
            .filter(|c| c.node_id.trim_start_matches("doc-").parse::<i64>().unwrap() % 5 == 0)
            .count();
        assert_eq!(
            heavy_hitters, 4,
            "the cap must keep the highest-fts_score rows, not an arbitrary subset: {capped:?}"
        );
    }

    /// Does Turso's ranked `fts_score`/`fts_match` path accept Tantivy's `term^boost`
    /// query syntax? `turso_core`'s FTS layer passes the bound `?1` string verbatim to
    /// Tantivy's own query parser, which documents `^boostfactor` — this confirms it
    /// empirically against the production statement. A down-weighted variant-only match
    /// must still score non-zero (not silently dropped or erroring) and must rank below
    /// the exact-term match.
    #[test]
    fn fts_boost_syntax_is_accepted_and_downweights() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index =
            Index::create_for_bulk_load(&dir.path().join("index.db")).expect("create index");
        index
            .execute(
                "INSERT INTO sections (node_id, heading, line, body) VALUES (?1, ?2, ?3, ?4)",
                turso::params![
                    "doc-exact",
                    "Overview",
                    1i64,
                    "We rename the field before publishing."
                ],
            )
            .expect("insert doc-exact");
        index
            .execute(
                "INSERT INTO sections (node_id, heading, line, body) VALUES (?1, ?2, ?3, ?4)",
                turso::params![
                    "doc-variant",
                    "Overview",
                    1i64,
                    "The renaming happened after publishing."
                ],
            )
            .expect("insert doc-variant");
        index.ensure_fts_index().expect("build fts index");

        // Mirrors what the real query builder produces for token "rename": the exact
        // token at full weight, a plausible variant down-weighted.
        let candidates = fts_candidates(&index, "rename renaming^0.5", Some(10))
            .expect("boosted query must parse, not hard-error");

        assert_eq!(
            candidates.len(),
            2,
            "both the exact-term doc and the variant-only doc must match: {candidates:?}"
        );
        assert!(
            candidates.iter().all(|c| c.tantivy_score > 0.0),
            "a boosted variant-only match must still score non-zero, not the silent \
             all-0.0 fallback shape: {candidates:?}"
        );
        let exact = candidates
            .iter()
            .find(|c| c.node_id == "doc-exact")
            .unwrap()
            .tantivy_score;
        let variant = candidates
            .iter()
            .find(|c| c.node_id == "doc-variant")
            .unwrap()
            .tantivy_score;
        assert!(
            exact > variant,
            "the exact-term match ({exact}) must outrank the down-weighted variant-only \
             match ({variant})"
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
