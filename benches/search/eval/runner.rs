//! Runs one corpus end-to-end: index it, warm up, time + score every query, assemble a
//! [`super::report::CorpusReport`]. Shared by `benches/search/main.rs` (every corpus) and
//! `tests/search_relevance.rs` (the `public` corpus only), so both exercise exactly the
//! same indexing/search/metrics path.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use vaire::embed::Embedder;
use vaire::index::build::{self, Mode};
use vaire::search::{SearchOpts, search};

use super::corpus::CorpusBuild;
use super::metrics::{self, HitRef};
use super::queries::{self, Query};
use super::report::{CorpusReport, QueryReport, TopHit};

pub const K: usize = 10;

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
    let repeat = repeat.max(1);

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
    if !missing.is_empty() {
        let msg = format!(
            "WARNING: {} judged id(s) in this corpus's queries do not exist in the {name} index: {}",
            missing.len(),
            missing.join(", ")
        );
        eprintln!("{msg}");
        notices.push(msg);
    }

    // One untimed warm-up pass over every query (lazy init, OS page cache, etc.).
    for q in queries {
        let _ = run_one_query(&index, embedder, &config, q);
    }

    let mut query_reports = Vec::with_capacity(queries.len());
    let mut evals: Vec<(String, metrics::QueryEval)> = Vec::new();
    let mut medians_ms = Vec::with_capacity(queries.len());

    for q in queries {
        let mut times_ms = Vec::with_capacity(repeat);
        let mut hits = Vec::new();
        for _ in 0..repeat {
            let start = Instant::now();
            hits = run_one_query(&index, embedder, &config, q)
                .map_err(|e| format!("query {:?} ({name} corpus): {e}", q.id))?;
            times_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let latency_ms_median = metrics::median(&mut times_ms);
        medians_ms.push(latency_ms_median);

        let hit_refs: Vec<HitRef> = hits
            .iter()
            .map(|h| HitRef {
                id: h.id.to_string(),
                node_type: h.node_type.to_string(),
            })
            .collect();
        let top10: Vec<TopHit> = hits
            .iter()
            .take(K)
            .map(|h| TopHit {
                id: h.id.to_string(),
                node_type: h.node_type.to_string(),
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
        node_count: summary.nodes,
        section_count,
        build_ms,
        overall,
        by_category,
        latency,
        queries: query_reports,
    })
}

fn run_one_query(
    index: &vaire::index::db::Index,
    embedder: &dyn Embedder,
    config: &vaire::config::Config,
    q: &Query,
) -> vaire::error::Result<Vec<vaire::search::SearchHit>> {
    let opts = SearchOpts {
        type_filter: q
            .type_filter
            .as_deref()
            .map(vaire::model::id::NodeType::new),
        limit: Some(K),
        scope_field: config.scope_field.clone(),
        ..Default::default()
    };
    search(index, embedder, &q.text, &opts)
}
