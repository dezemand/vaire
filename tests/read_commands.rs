//! Spec tests for the read commands (cli.md §3) over the standard fixture corpus.
//! Red until `index::build` + `index::query` are implemented (step 2).

mod common;

use common::Corpus;

use vaire::commands;
use vaire::error::ExitCode;
use vaire::output::Output;

// ---- resolve (cli.md §3.1) -------------------------------------------------

#[test]
fn resolve_returns_location_and_frontmatter() {
    let c = Corpus::fixture();
    let out = commands::resolve::run(&c.ctx(), "person:jane-doe").unwrap();

    assert_eq!(out.id, "person:jane-doe");
    assert_eq!(out.node_type, "person");
    assert_eq!(out.path, "knowledge/entities/people/jane-doe.md");
    assert_eq!(out.superseded_by, None);

    // The canonical JSON shape: id/type/path are top-level; frontmatter carries the
    // remaining fields but NOT id/type (cli.md §3.1 example).
    let json = out.to_json();
    assert_eq!(json["frontmatter"]["name"], "Jane Doe");
    assert!(json["frontmatter"].get("id").is_none());
    assert!(json["frontmatter"].get("type").is_none());
    assert_eq!(json["superseded_by"], serde_json::Value::Null);
}

#[test]
fn resolve_follows_superseded_redirect() {
    let c = Corpus::fixture();
    // Requesting the superseded id resolves to the target; the chain is reported,
    // and `requested_id` records what was asked (cli.md §3.1, design.md §8).
    let out = commands::resolve::run(&c.ctx(), "person:j-doe-dup").unwrap();

    assert_eq!(out.id, "person:jane-doe");
    assert_eq!(out.path, "knowledge/entities/people/jane-doe.md");
    assert_eq!(out.requested_id.as_deref(), Some("person:j-doe-dup"));
    assert_eq!(out.superseded_by.as_deref(), Some("person:jane-doe"));
}

#[test]
fn resolve_unknown_id_is_exit_5() {
    let c = Corpus::fixture();
    let err = commands::resolve::run(&c.ctx(), "person:nobody").unwrap_err();
    assert_eq!(err.exit_code(), ExitCode::IdNotFound);
}

// ---- backlinks (cli.md §3.2) -----------------------------------------------

#[test]
fn backlinks_report_inbound_edges_with_origin() {
    let c = Corpus::fixture();
    let out = commands::backlinks::run(&c.ctx(), "person:jane-doe", None, None).unwrap();

    // The broker-sync record references jane-doe (frontmatter `participants` + inline).
    assert!(
        out.backlinks
            .iter()
            .any(|b| b.id == "record:2026-06-10-broker-sync")
    );
    let ref_types: Vec<&str> = out.backlinks.iter().map(|b| b.ref_type.as_str()).collect();
    assert!(ref_types.contains(&"participants"));
    assert!(ref_types.contains(&"inline"));

    // A superseded_by redirect is NOT a backlink edge (design.md §8).
    assert!(!out.backlinks.iter().any(|b| b.id == "person:j-doe-dup"));
    assert_eq!(out.count, out.backlinks.len());
}

#[test]
fn backlinks_sorted_by_referencing_id_ascending() {
    let c = Corpus::fixture();
    // ingest-api is referenced by both the 06-08 decision and the 06-10 meeting note.
    let out = commands::backlinks::run(&c.ctx(), "system:ingest-api", None, None).unwrap();
    let ids: Vec<&str> = out.backlinks.iter().map(|b| b.id.as_str()).collect();

    let first_0608 = ids.iter().position(|i| i.contains("2026-06-08"));
    let first_0610 = ids.iter().position(|i| i.contains("2026-06-10"));
    assert!(first_0608.is_some() && first_0610.is_some());
    assert!(first_0608 < first_0610, "ids must be ascending: {ids:?}");
}

#[test]
fn backlinks_type_filter() {
    let c = Corpus::fixture();
    // No node of type `person` references jane-doe.
    let out = commands::backlinks::run(&c.ctx(), "person:jane-doe", Some("person"), None).unwrap();
    assert_eq!(out.count, 0);
}

// ---- refs (cli.md §3.3) ----------------------------------------------------

#[test]
fn refs_depth_one_returns_outbound_targets() {
    let c = Corpus::fixture();
    let out = commands::refs::run(&c.ctx(), "record:2026-06-10-broker-sync", 1, None).unwrap();
    let targets: Vec<&str> = out.refs.iter().map(|r| r.id.as_str()).collect();

    for expected in [
        "person:jane-doe",
        "department:logistics",
        "method:event-sourcing",
        "system:ingest-api",
        "project:atlas-2026-q2",
        "record:2026-06-08-ingest-decision",
    ] {
        assert!(
            targets.contains(&expected),
            "missing {expected} in {targets:?}"
        );
    }
    // Every depth-1 edge is at distance 1.
    assert!(out.refs.iter().all(|r| r.distance == Some(1)));
    assert_eq!(out.depth, 1);
}

#[test]
fn refs_excludes_unresolved_references() {
    let c = Corpus::fixture();
    let out = commands::refs::run(&c.ctx(), "record:2026-06-10-broker-sync", 1, None).unwrap();
    // Unresolved [[?...]] are never edges (cli.md §3.3) — the descriptors never appear.
    assert!(
        out.refs
            .iter()
            .all(|r| !r.id.contains("logistics contact") && !r.id.contains("broker thing"))
    );
}

#[test]
fn refs_depth_two_reaches_second_hop() {
    let c = Corpus::fixture();
    // broker-sync → person:jane-doe (hop 1) → dept:platform (hop 2, via jane-doe's org).
    let d1 = commands::refs::run(&c.ctx(), "record:2026-06-10-broker-sync", 1, None).unwrap();
    assert!(!d1.refs.iter().any(|r| r.id == "department:platform"));

    let d2 = commands::refs::run(&c.ctx(), "record:2026-06-10-broker-sync", 2, None).unwrap();
    let platform = d2.refs.iter().find(|r| r.id == "department:platform");
    assert!(platform.is_some(), "depth-2 should reach dept:platform");
    assert_eq!(platform.unwrap().distance, Some(2));

    // Sorted by (distance, id): all distance-1 entries precede all distance-2 entries.
    let distances: Vec<u32> = d2.refs.iter().filter_map(|r| r.distance).collect();
    assert!(
        distances.windows(2).all(|w| w[0] <= w[1]),
        "not sorted: {distances:?}"
    );
}

// ---- unresolved (cli.md §3.5) ----------------------------------------------

#[test]
fn unresolved_lists_loose_ends_with_type_guess() {
    let c = Corpus::fixture();
    let out = commands::unresolved::run(&c.ctx(), None, None).unwrap();

    assert_eq!(out.count, 2);
    let person = out
        .unresolved
        .iter()
        .find(|u| u.descriptor == "someone from logistics")
        .unwrap();
    assert_eq!(person.type_guess.as_deref(), Some("person"));
    assert_eq!(person.record, "record:2026-06-10-broker-sync");

    // `[[?: ...]]` has no type guess.
    let typeless = out
        .unresolved
        .iter()
        .find(|u| u.descriptor == "the broker thing")
        .unwrap();
    assert_eq!(typeless.type_guess, None);
}

#[test]
fn unresolved_type_filter_matches_guess_only() {
    let c = Corpus::fixture();
    // `--type person` matches the [[?person: ...]] but not the typeless [[?: ...]].
    let out = commands::unresolved::run(&c.ctx(), Some("person"), None).unwrap();
    assert_eq!(out.count, 1);
    assert_eq!(out.unresolved[0].type_guess.as_deref(), Some("person"));
}
