//! Spec tests for `vaire suggest <descriptor>` — descriptor → ranked existing IDs
//! (alias + name first, FTS backup; the §8 matching used for authoring).

mod common;

use common::Corpus;
use vaire::commands;

fn corpus() -> Corpus {
    let c = Corpus::empty();
    c.add("knowledge/jane-doe.md", "---\nid: jane-doe\ntype: person\nname: Jane Doe\n---\n# Jane Doe\n")
        .add("knowledge/jane-smith.md", "---\nid: jane-smith\ntype: person\nname: Jane Smith\n---\n# Jane Smith\n")
        .add(
            "knowledge/logistics.md",
            "---\nid: logistics\ntype: department\nname: Logistics\naliases: [logistics contact, Ops]\n---\n# Logistics\n",
        )
        .add(
            "knowledge/ingest.md",
            "---\nid: ingest-api\ntype: system\nname: Ingest API\naliases: [ingest]\n---\n# Ingest API\n",
        )
        .commit()
        .build();
    c
}

#[test]
fn exact_alias_ranks_first() {
    let c = corpus();
    let out = commands::suggest::run(&c.ctx(), "logistics contact", None, Some(5)).unwrap();
    assert_eq!(
        out.suggestions.first().map(|s| s.id.as_str()),
        Some("department:logistics")
    );
}

#[test]
fn matches_token_in_name() {
    let c = corpus();
    let out = commands::suggest::run(&c.ctx(), "jane", None, Some(5)).unwrap();
    let ids: Vec<&str> = out.suggestions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"person:jane-doe"));
    assert!(ids.contains(&"person:jane-smith"));
}

#[test]
fn type_filter_narrows() {
    let c = corpus();
    let people = commands::suggest::run(&c.ctx(), "jane", Some("person"), Some(5)).unwrap();
    assert!(people.suggestions.iter().all(|s| s.node_type == "person"));
    // A department alias, restricted to person → nothing.
    let none =
        commands::suggest::run(&c.ctx(), "logistics contact", Some("person"), Some(5)).unwrap();
    assert_eq!(none.count, 0);
}

#[test]
fn limit_caps_suggestions() {
    let c = corpus();
    let out = commands::suggest::run(&c.ctx(), "jane", None, Some(1)).unwrap();
    assert_eq!(out.suggestions.len(), 1);
}

#[test]
fn no_match_is_empty() {
    let c = corpus();
    let out = commands::suggest::run(&c.ctx(), "zzqxnomatchqq", None, Some(5)).unwrap();
    assert_eq!(out.count, 0);
}
