//! Runs one corpus end-to-end: index it, warm up, time + score every query, assemble a
//! [`super::report::CorpusReport`]. Shared by `benches/search/main.rs` (every corpus) and
//! `tests/search_relevance.rs` (the `public` corpus only), so both exercise exactly the
//! same indexing/search/metrics path.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use vaire::embed::Embedder;
use vaire::index::build::{self, Mode};
use vaire::search::{SearchMember, SearchOpts, search, search_members, search_prepared};

use super::corpus::CorpusBuild;
use super::metrics::{self, HitRef};
use super::queries::{self, Query};
use super::report::{CorpusReport, QueryReport, TopHit};

pub const K: usize = 10;

/// One ranked hit as the metrics read it.
struct Found {
    /// The id as the judgments name it (see [`bare_id`]).
    id: String,
    node_type: String,
    score: f32,
}

/// Index size and build time, reported alongside the quality numbers.
struct Sizes {
    node_count: usize,
    section_count: usize,
    build_ms: f64,
}

/// Build `build`'s corpus into an index with `embedder`, then time + score `queries`
/// against it. `repeat` is how many timed passes each query gets (a median is taken); one
/// untimed warm-up pass runs first over every query.
pub fn run_corpus(
    name: &str,
    build_corpus: &CorpusBuild,
    embedder: &dyn Embedder,
    queries: &[Query],
    mut notices: Vec<String>,
    repeat: usize,
) -> Result<CorpusReport, String> {
    let repo = build_corpus.repo();
    let config = build_corpus.config();

    let started = Instant::now();
    let summary = build::run(&repo, &config, embedder, Mode::Full)
        .map_err(|e| format!("indexing {name} corpus: {e}"))?;
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    // Prefer the summary's own measurement (it starts before this harness's `Instant`, so
    // it's the more honest number); this local one only backstops it if that's ever absent.
    let build_ms = if summary.elapsed_ms > 0 {
        summary.elapsed_ms as f64
    } else {
        build_ms
    };

    let index = vaire::index::db::Index::open(&repo.index_db())
        .map_err(|e| format!("opening {name} index: {e}"))?;
    let section_count = index
        .scalar_i64("SELECT count(*) FROM sections", ())
        .map_err(|e| format!("counting sections in {name} index: {e}"))?
        as usize;

    let missing = queries::warn_missing_ids(&index, queries);
    push_missing_notice(name, &missing, &mut notices);

    let sizes = Sizes {
        node_count: summary.nodes,
        section_count,
        build_ms,
    };
    score_queries(name, queries, repeat, notices, sizes, |q| {
        let hits = search(
            &index,
            embedder,
            &q.text,
            &search_opts(q, &config.scope_field),
        )?;
        Ok(hits
            .into_iter()
            .map(|h| Found {
                id: h.id.to_string(),
                node_type: h.node_type.to_string(),
                score: h.score,
            })
            .collect())
    })
}

/// Spread `build`'s corpus over `parts` packages ([`super::corpus::split`]), index each part,
/// and score `queries` against the whole corpus's judgments twice — once per way of ranking a
/// search across packages:
///
/// - `<name>-split<N>-merged`: each part searched on its own, then every hit merged by score
///   (ties by qualified id) — how a search across linked packages was ranked before.
/// - `<name>-split<N>-pooled`: all parts ranked as one search (`vaire::search::search_members`).
///
/// Part 0 plays the package the search runs in; the others are its dependencies. Hits are
/// scored by their id without the `@part/` qualifier, which the split kept unique.
pub fn run_split_corpus(
    name: &str,
    build_corpus: &CorpusBuild,
    parts: usize,
    embedder: &dyn Embedder,
    queries: &[Query],
    mut notices: Vec<String>,
    repeat: usize,
) -> Result<Vec<CorpusReport>, String> {
    let splits = super::corpus::split(build_corpus, parts)?;

    let started = Instant::now();
    let mut node_count = 0;
    let mut section_count = 0;
    let mut indexes = Vec::with_capacity(splits.len());
    let mut names = Vec::with_capacity(splits.len());
    let mut missing: Option<BTreeSet<String>> = None;
    for part in &splits {
        notices.extend(part.notices.iter().cloned());
        let repo = part.repo();
        let config = part.config();
        let summary = build::run(&repo, &config, embedder, Mode::Full)
            .map_err(|e| format!("indexing {}: {e}", config.name))?;
        let index = vaire::index::db::Index::open(&repo.index_db())
            .map_err(|e| format!("opening {}: {e}", config.name))?;
        section_count += index
            .scalar_i64("SELECT count(*) FROM sections", ())
            .map_err(|e| format!("counting sections in {}: {e}", config.name))?
            as usize;
        node_count += summary.nodes;
        // A judged id is missing only if no part has it.
        let here: BTreeSet<String> = queries::warn_missing_ids(&index, queries)
            .into_iter()
            .collect();
        missing = Some(match missing {
            Some(so_far) => so_far.intersection(&here).cloned().collect(),
            None => here,
        });
        indexes.push(index);
        names.push(config.name);
    }
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let missing: Vec<String> = missing.unwrap_or_default().into_iter().collect();
    push_missing_notice(name, &missing, &mut notices);

    let members: Vec<SearchMember<'_>> = indexes
        .iter()
        .zip(&names)
        .enumerate()
        .map(|(i, (index, package))| SearchMember {
            package: (i > 0).then_some(package.as_str()),
            index,
        })
        .collect();
    let scope_field = splits
        .first()
        .map(|part| part.config().scope_field)
        .unwrap_or_default();

    let sizes = || Sizes {
        node_count,
        section_count,
        build_ms,
    };
    let merged = score_queries(
        &format!("{name}-split{parts}-merged"),
        queries,
        repeat,
        notices.clone(),
        sizes(),
        |q| merged_search(&members, embedder, q, &scope_field),
    )?;
    let pooled = score_queries(
        &format!("{name}-split{parts}-pooled"),
        queries,
        repeat,
        notices,
        sizes(),
        |q| {
            let opts = search_opts(q, &scope_field);
            Ok(search_members(&members, embedder, &q.text, &opts)?
                .into_iter()
                .map(|found| Found {
                    id: bare_id(&found.hit.id.to_string()),
                    node_type: found.hit.node_type.to_string(),
                    score: found.hit.score,
                })
                .collect())
        },
    )?;
    Ok(vec![merged, pooled])
}

/// A search across `members` ranked the way it was before members were ranked as one: each
/// member searched on its own, the hits merged by score, ties by qualified id, then the limit.
fn merged_search(
    members: &[SearchMember<'_>],
    embedder: &dyn Embedder,
    q: &Query,
    scope_field: &str,
) -> vaire::error::Result<Vec<Found>> {
    let opts = search_opts(q, scope_field);
    let qvec = embedder
        .embed(std::slice::from_ref(&q.text))?
        .into_iter()
        .next()
        .filter(|v| v.iter().any(|x| *x != 0.0));
    let mut all: Vec<(String, Found)> = Vec::new();
    for member in members {
        for hit in search_prepared(member.index, qvec.as_deref(), &q.text, &opts)? {
            let id = hit.id.to_string();
            let qualified = match member.package {
                Some(package) => format!("@{package}/{id}"),
                None => id.clone(),
            };
            all.push((
                qualified,
                Found {
                    id,
                    node_type: hit.node_type.to_string(),
                    score: hit.score,
                },
            ));
        }
    }
    all.sort_by(|(xq, x), (yq, y)| {
        y.score
            .partial_cmp(&x.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| xq.cmp(yq))
    });
    all.truncate(K);
    Ok(all.into_iter().map(|(_, found)| found).collect())
}

/// An id as the judgments name it: without the `@package/` qualifier a hit from a split
/// corpus's other parts carries.
fn bare_id(id: &str) -> String {
    match id.strip_prefix('@').and_then(|rest| rest.split_once('/')) {
        Some((_, bare)) => bare.to_string(),
        None => id.to_string(),
    }
}

fn push_missing_notice(name: &str, missing: &[String], notices: &mut Vec<String>) {
    if missing.is_empty() {
        return;
    }
    let msg = format!(
        "WARNING: {} judged id(s) in this corpus's queries do not exist in the {name} index: {}",
        missing.len(),
        missing.join(", ")
    );
    eprintln!("{msg}");
    notices.push(msg);
}

/// Warm up, then time + score every query through `search`, and assemble the report.
fn score_queries(
    name: &str,
    queries: &[Query],
    repeat: usize,
    notices: Vec<String>,
    sizes: Sizes,
    search: impl Fn(&Query) -> vaire::error::Result<Vec<Found>>,
) -> Result<CorpusReport, String> {
    let repeat = repeat.max(1);

    // One untimed warm-up pass over every query (lazy init, OS page cache, etc.).
    for q in queries {
        let _ = search(q);
    }

    let mut query_reports = Vec::with_capacity(queries.len());
    let mut evals: Vec<(String, metrics::QueryEval)> = Vec::new();
    let mut medians_ms = Vec::with_capacity(queries.len());

    for q in queries {
        let mut times_ms = Vec::with_capacity(repeat);
        let mut hits = Vec::new();
        for _ in 0..repeat {
            let start = Instant::now();
            hits = search(q).map_err(|e| format!("query {:?} ({name} corpus): {e}", q.id))?;
            times_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let latency_ms_median = metrics::median(&mut times_ms);
        medians_ms.push(latency_ms_median);

        let hit_refs: Vec<HitRef> = hits
            .iter()
            .map(|h| HitRef {
                id: h.id.clone(),
                node_type: h.node_type.clone(),
            })
            .collect();
        let top10: Vec<TopHit> = hits
            .iter()
            .take(K)
            .map(|h| TopHit {
                id: h.id.clone(),
                node_type: h.node_type.clone(),
                score: h.score,
            })
            .collect();

        let judged = q.is_judged();
        // `metrics::QueryEval` is `Copy`, so this stays available below after the push.
        let qe: Option<metrics::QueryEval> =
            judged.then(|| metrics::evaluate(&hit_refs, &q.relevant, K));
        if let Some(eval) = qe {
            evals.push((q.category.clone(), eval));
        }

        query_reports.push(QueryReport {
            id: q.id.clone(),
            category: q.category.clone(),
            text: q.text.clone(),
            type_filter: q.type_filter.clone(),
            judged,
            primary_expected_id: q.primary_expected_id().map(str::to_string),
            first_rel_rank: qe.and_then(|e| e.first_rel_rank),
            primary_rank: qe.and_then(|e| e.primary_rank),
            rr_at_10: qe.map(|e| e.rr_at_10),
            ndcg_at_10: qe.map(|e| e.ndcg_at_10),
            recall_at_10: qe.map(|e| e.recall_at_10),
            success_at_1: qe.map(|e| e.success_at_1),
            success_at_3: qe.map(|e| e.success_at_3),
            primary_at_1: qe.map(|e| e.primary_at_1),
            doc_intrusion_at_1: qe.map(|e| e.doc_intrusion_at_1),
            latency_ms_median,
            top10,
        });
    }

    let overall = metrics::aggregate(evals.iter().map(|(_, e)| e));
    let categories: BTreeSet<String> = evals.iter().map(|(c, _)| c.clone()).collect();
    let by_category: BTreeMap<String, metrics::Aggregate> = categories
        .into_iter()
        .map(|cat| {
            let agg = metrics::aggregate(evals.iter().filter(|(c, _)| *c == cat).map(|(_, e)| e));
            (cat, agg)
        })
        .collect();
    let latency = metrics::latency_stats(&medians_ms, repeat);

    Ok(CorpusReport {
        name: name.to_string(),
        notices,
        node_count: sizes.node_count,
        section_count: sizes.section_count,
        build_ms: sizes.build_ms,
        overall,
        by_category,
        latency,
        queries: query_reports,
    })
}

fn search_opts(q: &Query, scope_field: &str) -> SearchOpts {
    SearchOpts {
        type_filter: q
            .type_filter
            .as_deref()
            .map(vaire::model::id::NodeType::new),
        limit: Some(K),
        scope_field: scope_field.to_string(),
        ..Default::default()
    }
}
