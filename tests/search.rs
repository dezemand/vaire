//! Spec tests for `vaire search` (cli.md §3.4) over the standard fixture corpus.
//!
//! These pin the **precision** path (FTS + aliases) and the filters/ranking contract.
//! The vector-recall path is unit-tested in `search::vector`; with the placeholder
//! feature-hash embedder it is intentionally conservative, so these don't rely on it.

mod common;

use common::Corpus;
use vaire::commands;

#[test]
fn finds_prose_match_with_anchor() {
    let c = Corpus::fixture();
    let out = commands::search::run(&c.ctx(), "throughput", None, None, Some(10)).unwrap();

    // "throughput" appears only in the broker-sync note.
    let hit = out
        .results
        .iter()
        .find(|r| r.id == "record:2026-06-10-broker-sync")
        .expect("broker-sync should match");
    assert!(
        hit.anchors
            .iter()
            .any(|a| a.snippet.to_lowercase().contains("throughput"))
    );
    assert_eq!(out.count, out.results.len());
}

#[test]
fn alias_match_finds_entity() {
    let c = Corpus::fixture();
    // "logistics contact" is an alias of department:logistics; "contact" appears in no
    // prose, so only the alias path can surface it.
    let out = commands::search::run(&c.ctx(), "logistics contact", None, None, Some(10)).unwrap();
    assert!(out.results.iter().any(|r| r.id == "department:logistics"));
}

#[test]
fn type_filter_restricts_to_type() {
    let c = Corpus::fixture();
    let out =
        commands::search::run(&c.ctx(), "scope first", Some("record"), None, Some(10)).unwrap();
    assert!(!out.results.is_empty());
    assert!(out.results.iter().all(|r| r.node_type == "record"));
}

#[test]
fn scope_filter_keeps_only_project_records() {
    let c = Corpus::fixture();
    // "ingest" matches system:ingest-api (entity) and the records referencing it.
    // Scoped to the project, the entity drops out — only its records remain.
    let out = commands::search::run(
        &c.ctx(),
        "ingest",
        None,
        Some("project:atlas-2026-q2"),
        Some(10),
    )
    .unwrap();
    assert!(!out.results.is_empty());
    assert!(out.results.iter().all(|r| r.id != "system:ingest-api"));
    assert!(out.results.iter().any(|r| r.node_type == "record"));
}

#[test]
fn limit_caps_results() {
    let c = Corpus::fixture();
    // "scope" appears in both records.
    let out = commands::search::run(&c.ctx(), "scope", None, None, Some(1)).unwrap();
    assert_eq!(out.results.len(), 1);
    assert_eq!(out.count, 1);
}

#[test]
fn results_sorted_by_score_descending() {
    let c = Corpus::fixture();
    let out = commands::search::run(&c.ctx(), "ingest scope first", None, None, Some(10)).unwrap();
    assert!(out.results.windows(2).all(|w| w[0].score >= w[1].score));
}

#[test]
fn nonsense_query_returns_nothing() {
    let c = Corpus::fixture();
    let out = commands::search::run(&c.ctx(), "zzqxwvbnmlkjhgf", None, None, Some(10)).unwrap();
    assert_eq!(out.count, 0);
}
