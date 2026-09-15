//! Search-quality and latency metrics (k = 10) — `benches/search/README.md` §Metrics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A ranked hit, reduced to what metrics need (id + type; score isn't used for scoring
/// metrics, only reported for display).
#[derive(Debug, Clone)]
pub struct HitRef {
    pub id: String,
    pub node_type: String,
}

/// Per-query evaluation against one query's judgments, at cutoff `k`.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryEval {
    /// 1-based rank of the first hit with grade >= 1 in the top `k`, `None` if absent.
    pub first_rel_rank: Option<usize>,
    /// 1-based rank of the first hit with grade == 2 (the primary answer) in the top `k`.
    pub primary_rank: Option<usize>,
    pub rr_at_10: f64,
    pub ndcg_at_10: f64,
    pub recall_at_10: f64,
    pub success_at_1: bool,
    pub success_at_3: bool,
    pub primary_at_1: bool,
    /// 1 iff the top-1 hit is a `document` that is not itself judged relevant for this query
    /// — the issue #52 symptom, measured directly.
    pub doc_intrusion_at_1: bool,
}

/// Evaluate one query's ranked `hits` (already limited to the search call's own `limit`,
/// but defensively re-truncated to `k` here) against its `relevant` judgments.
pub fn evaluate(hits: &[HitRef], relevant: &BTreeMap<String, u8>, k: usize) -> QueryEval {
    let top_k = &hits[..hits.len().min(k)];

    let mut first_rel_rank = None;
    let mut primary_rank = None;
    for (i, hit) in top_k.iter().enumerate() {
        let grade = relevant.get(&hit.id).copied().unwrap_or(0);
        if grade >= 1 && first_rel_rank.is_none() {
            first_rel_rank = Some(i + 1);
        }
        if grade == 2 && primary_rank.is_none() {
            primary_rank = Some(i + 1);
        }
    }

    let rr_at_10 = first_rel_rank.map(|r| 1.0 / r as f64).unwrap_or(0.0);
    let ndcg_at_10 = ndcg(top_k, relevant, k);

    let relevant_ids_found = top_k
        .iter()
        .filter(|h| relevant.contains_key(&h.id))
        .count();
    let recall_at_10 = if relevant.is_empty() {
        0.0
    } else {
        relevant_ids_found as f64 / relevant.len() as f64
    };

    // Judged per hit, not per query: a query may legitimately accept one spec as a secondary
    // answer while a different, unjudged spec is what actually crowds out the primary.
    let doc_intrusion_at_1 = hits
        .first()
        .is_some_and(|h| h.node_type == "document" && !relevant.contains_key(&h.id));

    QueryEval {
        first_rel_rank,
        primary_rank,
        rr_at_10,
        ndcg_at_10,
        recall_at_10,
        success_at_1: first_rel_rank.is_some_and(|r| r <= 1),
        success_at_3: first_rel_rank.is_some_and(|r| r <= 3),
        primary_at_1: primary_rank == Some(1),
        doc_intrusion_at_1,
    }
}

/// gain = 2^grade - 1, discount = log2(rank + 1); ideal DCG from the judgments themselves
/// (their grades sorted best-first), truncated to `k`.
fn ndcg(top_k: &[HitRef], relevant: &BTreeMap<String, u8>, k: usize) -> f64 {
    let dcg: f64 = top_k
        .iter()
        .enumerate()
        .map(|(i, hit)| {
            let grade = relevant.get(&hit.id).copied().unwrap_or(0) as f64;
            gain(grade) / discount(i)
        })
        .sum();

    let mut ideal_grades: Vec<u8> = relevant.values().copied().collect();
    ideal_grades.sort_unstable_by(|a, b| b.cmp(a));
    let idcg: f64 = ideal_grades
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &grade)| gain(grade as f64) / discount(i))
        .sum();

    if idcg > 0.0 { dcg / idcg } else { 0.0 }
}

fn gain(grade: f64) -> f64 {
    2f64.powf(grade) - 1.0
}

/// `i` is the 0-based position; rank = i + 1, discount = log2(rank + 1) = log2(i + 2).
fn discount(i: usize) -> f64 {
    ((i + 2) as f64).log2()
}

/// Mean of every metric across a set of judged-query evaluations. `n` is the count of
/// evaluations folded in (0 ⇒ every mean is 0.0, not NaN).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Aggregate {
    pub n: usize,
    pub mrr_at_10: f64,
    pub ndcg_at_10: f64,
    pub recall_at_10: f64,
    pub success_at_1: f64,
    pub success_at_3: f64,
    pub primary_at_1: f64,
    pub doc_intrusion_at_1: f64,
}

pub fn aggregate<'a>(evals: impl Iterator<Item = &'a QueryEval>) -> Aggregate {
    let mut sum = Aggregate::default();
    let mut n = 0usize;
    for e in evals {
        n += 1;
        sum.mrr_at_10 += e.rr_at_10;
        sum.ndcg_at_10 += e.ndcg_at_10;
        sum.recall_at_10 += e.recall_at_10;
        sum.success_at_1 += bool01(e.success_at_1);
        sum.success_at_3 += bool01(e.success_at_3);
        sum.primary_at_1 += bool01(e.primary_at_1);
        sum.doc_intrusion_at_1 += bool01(e.doc_intrusion_at_1);
    }
    if n == 0 {
        return Aggregate::default();
    }
    let d = n as f64;
    Aggregate {
        n,
        mrr_at_10: sum.mrr_at_10 / d,
        ndcg_at_10: sum.ndcg_at_10 / d,
        recall_at_10: sum.recall_at_10 / d,
        success_at_1: sum.success_at_1 / d,
        success_at_3: sum.success_at_3 / d,
        primary_at_1: sum.primary_at_1 / d,
        doc_intrusion_at_1: sum.doc_intrusion_at_1 / d,
    }
}

fn bool01(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

/// Descriptive stats (ms) over the per-query *median* latency of a corpus run — see
/// [`crate` docs]/`benches/search/README.md` §Metrics for the two-level (repeat, then
/// across-queries) aggregation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LatencyStats {
    pub repeat: usize,
    pub n_queries: usize,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// Sort `values` and return the median (average of the two middle elements when even).
pub fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// Linear-interpolation percentile over an already-ascending-sorted slice.
fn percentile(sorted_ascending: &[f64], p: f64) -> f64 {
    match sorted_ascending.len() {
        0 => 0.0,
        1 => sorted_ascending[0],
        len => {
            let rank = p * (len - 1) as f64;
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            if lo == hi {
                sorted_ascending[lo]
            } else {
                let frac = rank - lo as f64;
                sorted_ascending[lo] + (sorted_ascending[hi] - sorted_ascending[lo]) * frac
            }
        }
    }
}

/// Aggregate the per-query median latencies (ms) of one corpus run into mean/p50/p95/max.
pub fn latency_stats(medians_ms: &[f64], repeat: usize) -> LatencyStats {
    if medians_ms.is_empty() {
        return LatencyStats {
            repeat,
            ..Default::default()
        };
    }
    let mut sorted = medians_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean_ms = sorted.iter().sum::<f64>() / sorted.len() as f64;
    LatencyStats {
        repeat,
        n_queries: sorted.len(),
        mean_ms,
        p50_ms: percentile(&sorted, 0.50),
        p95_ms: percentile(&sorted, 0.95),
        max_ms: *sorted.last().expect("non-empty"),
    }
}

// See the comment on the `tests` module in `corpus.rs`: this file also compiles into the
// `harness = false` search bench, where these test-only items are unreferenced.
#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;

    fn hit(id: &str, ty: &str) -> HitRef {
        HitRef {
            id: id.to_string(),
            node_type: ty.to_string(),
        }
    }

    #[test]
    fn perfect_rank_one_primary_hit() {
        let hits = vec![
            hit("concept:loose-end", "concept"),
            hit("document:spec", "document"),
        ];
        let mut relevant = BTreeMap::new();
        relevant.insert("concept:loose-end".to_string(), 2u8);
        let e = evaluate(&hits, &relevant, 10);
        assert_eq!(e.first_rel_rank, Some(1));
        assert_eq!(e.primary_rank, Some(1));
        assert_eq!(e.rr_at_10, 1.0);
        assert!((e.ndcg_at_10 - 1.0).abs() < 1e-9);
        assert!(e.success_at_1);
        assert!(e.primary_at_1);
        assert!(!e.doc_intrusion_at_1);
    }

    #[test]
    fn doc_intrusion_flags_a_wrongly_top_ranked_document() {
        let hits = vec![
            hit("document:design-spec", "document"),
            hit("concept:loose-end", "concept"),
        ];
        let mut relevant = BTreeMap::new();
        relevant.insert("concept:loose-end".to_string(), 2u8);
        let e = evaluate(&hits, &relevant, 10);
        assert!(
            e.doc_intrusion_at_1,
            "top-1 is a document, no document:* judged relevant"
        );
        assert_eq!(e.first_rel_rank, Some(2));
        assert_eq!(e.rr_at_10, 0.5);
    }

    #[test]
    fn doc_intrusion_is_not_flagged_when_a_document_is_the_judged_answer() {
        let hits = vec![hit("document:cli-spec", "document")];
        let mut relevant = BTreeMap::new();
        relevant.insert("document:cli-spec".to_string(), 2u8);
        let e = evaluate(&hits, &relevant, 10);
        assert!(!e.doc_intrusion_at_1);
    }

    #[test]
    fn doc_intrusion_is_flagged_for_an_unjudged_document_even_when_another_is_judged() {
        let hits = vec![
            hit("document:cli-spec", "document"),
            hit("concept:loose-end", "concept"),
        ];
        let mut relevant = BTreeMap::new();
        relevant.insert("concept:loose-end".to_string(), 2u8);
        relevant.insert("document:design-spec".to_string(), 1u8);
        let e = evaluate(&hits, &relevant, 10);
        assert!(e.doc_intrusion_at_1);
    }

    #[test]
    fn absent_relevant_hit_yields_zero_metrics() {
        let hits = vec![hit("concept:other", "concept")];
        let mut relevant = BTreeMap::new();
        relevant.insert("concept:loose-end".to_string(), 2u8);
        let e = evaluate(&hits, &relevant, 10);
        assert_eq!(e.first_rel_rank, None);
        assert_eq!(e.rr_at_10, 0.0);
        assert_eq!(e.ndcg_at_10, 0.0);
        assert_eq!(e.recall_at_10, 0.0);
        assert!(!e.success_at_1 && !e.success_at_3 && !e.primary_at_1);
    }

    #[test]
    fn recall_counts_the_judged_ids_actually_retrieved() {
        let hits = vec![
            hit("a", "concept"),
            hit("b", "concept"),
            hit("c", "concept"),
        ];
        let mut relevant = BTreeMap::new();
        relevant.insert("a".to_string(), 1u8);
        relevant.insert("b".to_string(), 1u8);
        relevant.insert("z".to_string(), 1u8); // never retrieved
        let e = evaluate(&hits, &relevant, 10);
        assert!((e.recall_at_10 - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn median_and_percentile_basics() {
        let mut v = vec![3.0, 1.0, 2.0];
        assert_eq!(median(&mut v), 2.0);
        let mut v2 = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(median(&mut v2), 2.5);

        let stats = latency_stats(&[1.0, 2.0, 3.0, 4.0, 100.0], 5);
        assert_eq!(stats.n_queries, 5);
        assert_eq!(stats.max_ms, 100.0);
        assert!(stats.p50_ms >= 2.0 && stats.p50_ms <= 3.0);
    }

    #[test]
    fn aggregate_of_empty_is_all_zero_not_nan() {
        let empty: Vec<QueryEval> = Vec::new();
        let a = aggregate(empty.iter());
        assert_eq!(a.n, 0);
        assert_eq!(a.mrr_at_10, 0.0);
    }
}
