//! Report assembly: the JSON shape written to `<out>/<label>.json`, and the Markdown
//! rendered to stdout — both for a single run and for `--compare base.json new.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::metrics::{Aggregate, LatencyStats};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopHit {
    pub id: String,
    pub node_type: String,
    pub score: f32,
}

/// Everything measured for one query within one corpus run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryReport {
    pub id: String,
    pub category: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_filter: Option<String>,
    pub judged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_expected_id: Option<String>,
    pub first_rel_rank: Option<usize>,
    pub primary_rank: Option<usize>,
    pub rr_at_10: Option<f64>,
    pub ndcg_at_10: Option<f64>,
    pub recall_at_10: Option<f64>,
    pub success_at_1: Option<bool>,
    pub success_at_3: Option<bool>,
    pub primary_at_1: Option<bool>,
    pub doc_intrusion_at_1: Option<bool>,
    pub latency_ms_median: f64,
    pub top10: Vec<TopHit>,
}

/// Everything measured for one corpus within one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusReport {
    pub name: String,
    #[serde(default)]
    pub notices: Vec<String>,
    pub node_count: usize,
    pub section_count: usize,
    pub build_ms: f64,
    pub overall: Aggregate,
    pub by_category: BTreeMap<String, Aggregate>,
    pub latency: LatencyStats,
    pub queries: Vec<QueryReport>,
}

/// The full JSON report for one invocation, across every selected corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub label: String,
    pub git_branch: Option<String>,
    pub git_sha: Option<String>,
    pub embedder_identity: String,
    pub generated_at_unix_ms: u128,
    pub corpora: Vec<CorpusReport>,
}

/// Write `run` as pretty JSON to `<out_dir>/<run.label>.json`, creating `out_dir` if
/// needed. Returns the path written.
///
/// The label is sanitized to a single flat filename first — a `--label` (or a branch name
/// a caller forgot to flatten) can contain `/`, which would otherwise ask this to create
/// (or escape into) subdirectories of `out_dir`.
pub fn write_json(run: &RunReport, out_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("creating {}: {e}", out_dir.display()))?;
    let path = out_dir.join(format!("{}.json", sanitize_filename(&run.label)));
    let text = serde_json::to_string_pretty(run).map_err(|e| format!("serializing report: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

/// Flatten a label to characters safe as a single path component on every common
/// filesystem: alphanumerics, `-`, `_`, `.` pass through; everything else (`/`, `\`,
/// leading `.` runs that could read as `.`/`..`) becomes `-`.
fn sanitize_filename(label: &str) -> String {
    let flattened: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    match flattened.trim_start_matches('.') {
        "" => "unlabeled".to_string(),
        rest => rest.to_string(),
    }
}

pub fn read_json(path: &Path) -> Result<RunReport, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------
// Markdown — single run
// ---------------------------------------------------------------------------------

pub fn render_markdown(run: &RunReport, verbose: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Search benchmark — {}", run.label);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- branch: `{}`",
        run.git_branch.as_deref().unwrap_or("unknown")
    );
    let _ = writeln!(
        out,
        "- commit: `{}`",
        run.git_sha.as_deref().unwrap_or("unknown")
    );
    let _ = writeln!(out, "- embedder: `{}`", run.embedder_identity);
    let _ = writeln!(out, "- generated: {} (unix ms)", run.generated_at_unix_ms);

    for corpus in &run.corpora {
        render_corpus_markdown(&mut out, corpus, verbose);
    }
    out
}

fn render_corpus_markdown(out: &mut String, c: &CorpusReport, verbose: bool) {
    let _ = writeln!(out, "\n## Corpus: {}", c.name);
    for notice in &c.notices {
        let _ = writeln!(out, "> {notice}");
    }
    let _ = writeln!(
        out,
        "\n- nodes: {}, sections: {}, build: {:.3} ms",
        c.node_count, c.section_count, c.build_ms
    );

    let _ = writeln!(out, "\n### Overall");
    write_metrics_table_header(out);
    write_metrics_row(out, "overall", &c.overall);

    let _ = writeln!(out, "\n### By category");
    write_metrics_table_header(out);
    for (cat, agg) in &c.by_category {
        write_metrics_row(out, cat, agg);
    }
    if c.by_category.is_empty() {
        let _ = writeln!(out, "| _(no judged queries)_ | | | | | | | | |");
    }

    let _ = writeln!(
        out,
        "\n### Latency ({} repeat(s), {} quer(y/ies))",
        c.latency.repeat, c.latency.n_queries
    );
    let _ = writeln!(out, "| mean ms | p50 ms | p95 ms | max ms |");
    let _ = writeln!(out, "|---:|---:|---:|---:|");
    let _ = writeln!(
        out,
        "| {:.3} | {:.3} | {:.3} | {:.3} |",
        c.latency.mean_ms, c.latency.p50_ms, c.latency.p95_ms, c.latency.max_ms
    );

    if verbose {
        let _ = writeln!(out, "\n### Per-query");
        let _ = writeln!(
            out,
            "| id | category | query | primary expected | primary rank | first rel rank | top-1 | nDCG@10 |"
        );
        let _ = writeln!(out, "|---|---|---|---|---:|---:|---|---:|");
        for q in &c.queries {
            let top1 = q.top10.first().map(|h| h.id.as_str()).unwrap_or("-");
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                md_escape(&q.id),
                md_escape(&q.category),
                md_escape(&truncate_chars(&q.text, 50)),
                q.primary_expected_id
                    .as_deref()
                    .map(md_escape)
                    .unwrap_or_else(|| "-".to_string()),
                opt_usize(q.primary_rank),
                opt_usize(q.first_rel_rank),
                md_escape(top1),
                opt_f64(q.ndcg_at_10),
            );
        }
    }
}

fn write_metrics_table_header(out: &mut String) {
    let _ = writeln!(
        out,
        "| set | n | MRR@10 | nDCG@10 | Recall@10 | Success@1 | Success@3 | Primary@1 | DocIntrusion@1 |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
}

fn write_metrics_row(out: &mut String, label: &str, a: &Aggregate) {
    let _ = writeln!(
        out,
        "| {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} |",
        md_escape(label),
        a.n,
        a.mrr_at_10,
        a.ndcg_at_10,
        a.recall_at_10,
        a.success_at_1,
        a.success_at_3,
        a.primary_at_1,
        a.doc_intrusion_at_1
    );
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Escape the handful of characters that would otherwise break a Markdown table cell.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn opt_usize(v: Option<usize>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string())
}

fn opt_f64(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.3}"))
        .unwrap_or_else(|| "-".to_string())
}

// ---------------------------------------------------------------------------------
// Markdown — `--compare base.json new.json`
// ---------------------------------------------------------------------------------

pub fn render_compare_markdown(base: &RunReport, new: &RunReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Search benchmark comparison");
    let _ = writeln!(
        out,
        "\n- base: `{}` (branch `{}`, sha `{}`)",
        base.label,
        base.git_branch.as_deref().unwrap_or("?"),
        base.git_sha.as_deref().unwrap_or("?")
    );
    let _ = writeln!(
        out,
        "- new:  `{}` (branch `{}`, sha `{}`)",
        new.label,
        new.git_branch.as_deref().unwrap_or("?"),
        new.git_sha.as_deref().unwrap_or("?")
    );
    if base.embedder_identity != new.embedder_identity {
        let _ = writeln!(
            out,
            "> WARNING: embedder identity differs (base `{}` vs. new `{}`) — scores may not be comparable",
            base.embedder_identity, new.embedder_identity
        );
    }

    let mut base_by_name: BTreeMap<&str, &CorpusReport> =
        base.corpora.iter().map(|c| (c.name.as_str(), c)).collect();

    for new_c in &new.corpora {
        let _ = writeln!(out, "\n## Corpus: {}", new_c.name);
        let Some(base_c) = base_by_name.remove(new_c.name.as_str()) else {
            let _ = writeln!(out, "> only present in `new` (no matching corpus in base)");
            continue;
        };
        render_corpus_compare(&mut out, base_c, new_c);
    }
    for name in base_by_name.keys() {
        let _ = writeln!(out, "\n## Corpus: {name}");
        let _ = writeln!(out, "> only present in `base` (no matching corpus in new)");
    }
    out
}

fn render_corpus_compare(out: &mut String, b: &CorpusReport, n: &CorpusReport) {
    let _ = writeln!(
        out,
        "\n- nodes: {} -> {}, sections: {} -> {}, build: {:.3} -> {:.3} ms ({:+.3})",
        b.node_count,
        n.node_count,
        b.section_count,
        n.section_count,
        b.build_ms,
        n.build_ms,
        n.build_ms - b.build_ms
    );

    let _ = writeln!(out, "\n### Overall (base -> new (delta))");
    write_delta_table_header(out);
    write_delta_row(out, "overall", &b.overall, &n.overall);

    let _ = writeln!(out, "\n### By category (base -> new (delta))");
    write_delta_table_header(out);
    let categories: BTreeSet<&str> = b
        .by_category
        .keys()
        .map(String::as_str)
        .chain(n.by_category.keys().map(String::as_str))
        .collect();
    for cat in categories {
        match (b.by_category.get(cat), n.by_category.get(cat)) {
            (Some(bb), Some(nn)) => write_delta_row(out, cat, bb, nn),
            (Some(bb), None) => write_delta_row(out, cat, bb, &Aggregate::default()),
            (None, Some(nn)) => write_delta_row(out, cat, &Aggregate::default(), nn),
            (None, None) => {}
        }
    }

    let _ = writeln!(out, "\n### Latency (ms)");
    let _ = writeln!(out, "| stat | base | new | delta |");
    let _ = writeln!(out, "|---|---:|---:|---:|");
    for (label, bv, nv) in [
        ("mean", b.latency.mean_ms, n.latency.mean_ms),
        ("p50", b.latency.p50_ms, n.latency.p50_ms),
        ("p95", b.latency.p95_ms, n.latency.p95_ms),
        ("max", b.latency.max_ms, n.latency.max_ms),
    ] {
        let _ = writeln!(out, "| {label} | {bv:.3} | {nv:.3} | {:+.3} |", nv - bv);
    }

    let base_by_id: BTreeMap<&str, &QueryReport> =
        b.queries.iter().map(|q| (q.id.as_str(), q)).collect();
    let mut improved = Vec::new();
    let mut regressed = Vec::new();
    for nq in &n.queries {
        let Some(bq) = base_by_id.get(nq.id.as_str()) else {
            continue;
        };
        match (bq.primary_rank, nq.primary_rank) {
            (Some(bp), Some(np)) if np < bp => improved.push(format!("`{}`: {bp} -> {np}", nq.id)),
            (Some(bp), Some(np)) if np > bp => regressed.push(format!("`{}`: {bp} -> {np}", nq.id)),
            (None, Some(np)) => improved.push(format!("`{}`: (absent) -> {np}", nq.id)),
            (Some(bp), None) => regressed.push(format!("`{}`: {bp} -> (absent)", nq.id)),
            _ => {}
        }
    }
    let _ = writeln!(out, "\n### Primary-rank changes");
    if improved.is_empty() && regressed.is_empty() {
        let _ = writeln!(out, "- no primary-rank changes");
    } else {
        let _ = writeln!(
            out,
            "- improved ({}): {}",
            improved.len(),
            if improved.is_empty() {
                "-".to_string()
            } else {
                improved.join("; ")
            }
        );
        let _ = writeln!(
            out,
            "- regressed ({}): {}",
            regressed.len(),
            if regressed.is_empty() {
                "-".to_string()
            } else {
                regressed.join("; ")
            }
        );
    }
}

fn write_delta_table_header(out: &mut String) {
    let _ = writeln!(
        out,
        "| set | n | MRR@10 | nDCG@10 | Recall@10 | Success@1 | Success@3 | Primary@1 | DocIntrusion@1 |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|");
}

fn write_delta_row(out: &mut String, label: &str, b: &Aggregate, n: &Aggregate) {
    let fmt = |bv: f64, nv: f64| format!("{bv:.3} -> {nv:.3} ({:+.3})", nv - bv);
    let _ = writeln!(
        out,
        "| {} | {} -> {} | {} | {} | {} | {} | {} | {} | {} |",
        md_escape(label),
        b.n,
        n.n,
        fmt(b.mrr_at_10, n.mrr_at_10),
        fmt(b.ndcg_at_10, n.ndcg_at_10),
        fmt(b.recall_at_10, n.recall_at_10),
        fmt(b.success_at_1, n.success_at_1),
        fmt(b.success_at_3, n.success_at_3),
        fmt(b.primary_at_1, n.primary_at_1),
        fmt(b.doc_intrusion_at_1, n.doc_intrusion_at_1),
    );
}

// See the comment on the `tests` module in `corpus.rs`: this file also compiles into the
// `harness = false` search bench, where these test-only items are unreferenced.
#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;

    fn sample_run(label: &str) -> RunReport {
        RunReport {
            label: label.to_string(),
            git_branch: Some("main".to_string()),
            git_sha: Some("abc1234".to_string()),
            embedder_identity: "local:384".to_string(),
            generated_at_unix_ms: 0,
            corpora: vec![CorpusReport {
                name: "public".to_string(),
                notices: Vec::new(),
                node_count: 10,
                section_count: 20,
                build_ms: 5.0,
                overall: Aggregate {
                    n: 1,
                    mrr_at_10: 1.0,
                    ndcg_at_10: 1.0,
                    recall_at_10: 1.0,
                    success_at_1: 1.0,
                    success_at_3: 1.0,
                    primary_at_1: 1.0,
                    doc_intrusion_at_1: 0.0,
                },
                by_category: BTreeMap::new(),
                latency: LatencyStats {
                    repeat: 5,
                    n_queries: 1,
                    mean_ms: 1.0,
                    p50_ms: 1.0,
                    p95_ms: 1.0,
                    max_ms: 1.0,
                },
                queries: vec![QueryReport {
                    id: "q1".to_string(),
                    category: "name".to_string(),
                    text: "loose end".to_string(),
                    type_filter: None,
                    judged: true,
                    primary_expected_id: Some("concept:loose-end".to_string()),
                    first_rel_rank: Some(1),
                    primary_rank: Some(1),
                    rr_at_10: Some(1.0),
                    ndcg_at_10: Some(1.0),
                    recall_at_10: Some(1.0),
                    success_at_1: Some(true),
                    success_at_3: Some(true),
                    primary_at_1: Some(true),
                    doc_intrusion_at_1: Some(false),
                    latency_ms_median: 1.0,
                    top10: vec![TopHit {
                        id: "concept:loose-end".to_string(),
                        node_type: "concept".to_string(),
                        score: 5.0,
                    }],
                }],
            }],
        }
    }

    #[test]
    fn json_round_trips() {
        let run = sample_run("t-1");
        let dir = tempfile::tempdir().unwrap();
        let path = write_json(&run, dir.path()).unwrap();
        assert_eq!(path, dir.path().join("t-1.json"));
        let back = read_json(&path).unwrap();
        assert_eq!(back.label, "t-1");
        assert_eq!(back.corpora[0].queries[0].id, "q1");
    }

    #[test]
    fn markdown_renders_without_panicking_and_contains_key_facts() {
        let run = sample_run("t-1");
        let md = render_markdown(&run, true);
        assert!(md.contains("# Search benchmark — t-1"));
        assert!(md.contains("Corpus: public"));
        assert!(md.contains("loose end"));
    }

    #[test]
    fn compare_reports_a_primary_rank_regression() {
        let base = sample_run("base");
        let mut new = sample_run("new");
        new.corpora[0].queries[0].primary_rank = Some(3);
        let md = render_compare_markdown(&base, &new);
        assert!(md.contains("regressed (1)"));
        assert!(md.contains("1 -> 3"));
    }
}
