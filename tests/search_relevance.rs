//! Sanity check for the issue #52 search benchmark harness: builds the `public` corpus
//! with the local embedder, runs its queries (if any exist yet), and prints the same
//! Markdown metrics table `cargo bench --bench search` does.
//!
//! Run with `cargo test --test search_relevance -- --nocapture` to see the table.
//!
//! Deliberately weak assertions for now (real thresholds land once the ranking
//! optimisations on other branches have something to be measured against): only that the
//! queries file parses to at least one query when it exists, and that every metric is
//! finite and within `[0, 1]`.

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

    if !queries_path.is_file() {
        println!(
            "search_relevance: {} does not exist yet — skipping (nothing to assert)",
            queries_path.display()
        );
        return;
    }

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
}
