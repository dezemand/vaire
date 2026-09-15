//! CI relevance floor for `vaire search` (issue #52): builds the `public` corpus with the
//! local embedder (CI has no network, so this is the only embedder these floors can hold
//! to a standard against), runs its queries, and prints the same Markdown metrics table
//! `cargo bench --bench search` does.
//!
//! Run with `cargo test --test search_relevance -- --nocapture` to see the table.
//!
//! Every metric is finite and within `[0, 1]` (basic sanity); overall MRR@10/nDCG@10 must
//! not drop far below, and DocIntrusion@1 not rise far above, the ranking pipeline's
//! measured numbers on this corpus; and a handful of exact name/alias queries must keep
//! ranking their answer first — the failure mode issue #52 is about, held to a standard
//! directly instead of only through an aggregate average.

// `eval` is the shared library the `search` bench also builds on (see
// `benches/search/README.md`); this test only exercises a slice of its surface (the
// `public` corpus with the `local` embedder), so the rest is legitimately unreached from
// here — the bench target (`cargo check --bench search`), which uses the full surface,
// is what actually guards against real dead code in this module.
#[path = "../benches/search/eval/mod.rs"]
#[allow(dead_code)]
mod eval;

use std::path::PathBuf;

#[test]
fn public_corpus_relevance_sanity() {
    eval::embedders::hermetic_config_home_unless_openai(eval::embedders::EmbedderKind::Local)
        .expect("hermetic config home");

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let queries_path = repo_root.join("benches/search/data/public/queries.toml");

    assert!(
        queries_path.is_file(),
        "{} is missing: the relevance floors below need it",
        queries_path.display()
    );

    let queries =
        eval::queries::load(&queries_path).expect("benches/search/data/public/queries.toml parses");
    assert!(
        !queries.is_empty(),
        "{} exists but loaded zero queries",
        queries_path.display()
    );

    let build = eval::corpus::build_public(&repo_root, None).expect("build the public corpus");
    let embedder = eval::embedders::EmbedderHandle::local().expect("build the local embedder");

    let report = eval::runner::run_corpus(
        "public",
        &build,
        embedder.as_dyn(),
        &queries,
        build.notices.clone(),
        1, // one repeat is plenty for a sanity check; this test isn't measuring latency
    )
    .expect("run the public corpus");

    let run_report = eval::report::RunReport {
        label: "search_relevance-test".to_string(),
        git_branch: None,
        git_sha: None,
        embedder_identity: embedder.as_dyn().identity(),
        generated_at_unix_ms: 0,
        corpora: vec![report.clone()],
    };
    println!("{}", eval::report::render_markdown(&run_report, true));

    let finite_unit = |name: &str, v: f64| {
        assert!(
            v.is_finite() && (0.0..=1.0).contains(&v),
            "{name} must be finite and within [0,1], got {v}"
        );
    };

    finite_unit("overall.mrr_at_10", report.overall.mrr_at_10);
    finite_unit("overall.ndcg_at_10", report.overall.ndcg_at_10);
    finite_unit("overall.recall_at_10", report.overall.recall_at_10);
    finite_unit("overall.success_at_1", report.overall.success_at_1);
    finite_unit("overall.success_at_3", report.overall.success_at_3);
    finite_unit("overall.primary_at_1", report.overall.primary_at_1);
    finite_unit(
        "overall.doc_intrusion_at_1",
        report.overall.doc_intrusion_at_1,
    );

    for agg in report.by_category.values() {
        finite_unit("by_category.mrr_at_10", agg.mrr_at_10);
        finite_unit("by_category.ndcg_at_10", agg.ndcg_at_10);
        finite_unit("by_category.recall_at_10", agg.recall_at_10);
        finite_unit("by_category.success_at_1", agg.success_at_1);
        finite_unit("by_category.success_at_3", agg.success_at_3);
        finite_unit("by_category.primary_at_1", agg.primary_at_1);
        finite_unit("by_category.doc_intrusion_at_1", agg.doc_intrusion_at_1);
    }

    for q in &report.queries {
        if !q.judged {
            continue;
        }
        finite_unit(
            &format!("query {:?} rr_at_10", q.id),
            q.rr_at_10.expect("judged"),
        );
        finite_unit(
            &format!("query {:?} ndcg_at_10", q.id),
            q.ndcg_at_10.expect("judged"),
        );
        finite_unit(
            &format!("query {:?} recall_at_10", q.id),
            q.recall_at_10.expect("judged"),
        );
    }

    // Relevance floors: the ranking pipeline's own measured numbers on this corpus, with
    // the local embedder, are MRR@10 0.694, nDCG@10 0.669, DocIntrusion@1 0.067. A 0.03
    // cushion on MRR/nDCG and 0.05 on DocIntrusion@1 catches a real ranking regression
    // without flaking on noise from an incidental change to the fixture corpus.
    const FLOOR_MRR: f64 = 0.694 - 0.03;
    const FLOOR_NDCG: f64 = 0.669 - 0.03;
    const CEILING_DOC_INTRUSION: f64 = 0.067 + 0.05;
    assert!(
        report.overall.mrr_at_10 >= FLOOR_MRR,
        "overall MRR@10 regressed: {} < floor {FLOOR_MRR}",
        report.overall.mrr_at_10
    );
    assert!(
        report.overall.ndcg_at_10 >= FLOOR_NDCG,
        "overall nDCG@10 regressed: {} < floor {FLOOR_NDCG}",
        report.overall.ndcg_at_10
    );
    assert!(
        report.overall.doc_intrusion_at_1 <= CEILING_DOC_INTRUSION,
        "overall DocIntrusion@1 regressed: {} > ceiling {CEILING_DOC_INTRUSION}",
        report.overall.doc_intrusion_at_1
    );

    // Must-pass queries: an exact name/alias match must never lose to a longer, merely-
    // related document — issue #52's symptom, held to a standard directly rather than
    // only through an aggregate average that could hide a single bad regression.
    let must_rank_first = [
        ("name-gate-the-rare-act", "principle:gate-the-rare-act"),
        ("alias-tombstone", "concept:supersession"),
        ("name-loose-end", "concept:loose-end"),
        ("name-supersession", "concept:supersession"),
        ("name-knowledge-lifecycle", "concept:knowledge-lifecycle"),
        ("name-hybrid-search", "component:hybrid-search"),
    ];
    let by_id: std::collections::HashMap<&str, &eval::report::QueryReport> =
        report.queries.iter().map(|q| (q.id.as_str(), q)).collect();
    for (query_id, expected_id) in must_rank_first {
        let q = *by_id
            .get(query_id)
            .unwrap_or_else(|| panic!("must-pass query {query_id:?} not found in queries.toml"));
        assert_eq!(
            q.primary_expected_id.as_deref(),
            Some(expected_id),
            "must-pass query {query_id:?} no longer judges {expected_id:?} as its primary answer"
        );
        assert_eq!(
            q.primary_rank,
            Some(1),
            "must-pass query {query_id:?} must rank {expected_id:?} first, got rank {:?} (top hit: {:?})",
            q.primary_rank,
            q.top10.first().map(|h| h.id.as_str())
        );
    }
}
