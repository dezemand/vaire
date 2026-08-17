//! Diagram references become graph edges (issue #23, design.md §6): a `vaire/`-prefixed
//! link target inside a fenced diagram block or an external diagram file becomes a
//! `ref_type: diagram` edge — resolved the ordinary way, feeding backlinks/refs/dangling,
//! but never triggering `check`'s drift rule.

mod common;

use common::{Corpus, DummyEmbedder};
use vaire::commands;
use vaire::index::build::Mode;
use vaire::index::check::{Violation, Warning};

#[test]
fn fenced_plantuml_block_creates_a_diagram_edge_and_backlinks() {
    let c = Corpus::empty();
    c.add(
        "knowledge/gateway.md",
        "---\nid: gateway\ntype: system\nname: Gateway\n---\n# Gateway\n",
    )
    .add(
        "knowledge/arch.md",
        "---\nid: arch\ntype: record\n---\n# Architecture\n\n\
         ```plantuml\ncomponent \"API Gateway\" [[vaire/system:gateway]] #FFD966\n```\n",
    )
    .commit()
    .build();

    let refs = commands::refs::run(&c.ctx(), "record:arch", 1, None).unwrap();
    assert!(
        refs.refs
            .iter()
            .any(|r| r.id == "system:gateway" && r.ref_type == "diagram"),
        "expected a diagram edge, got {:?}",
        refs.refs
    );

    // An entity referenced only from a diagram must have non-empty backlinks — the
    // regression the issue calls out by name.
    let backlinks = commands::backlinks::run(&c.ctx(), "system:gateway", None, None).unwrap();
    assert!(
        !backlinks.backlinks.is_empty(),
        "diagram-only reference must still produce backlinks"
    );
}

#[test]
fn external_diagram_file_creates_a_diagram_edge_sourced_at_the_diagram() {
    let c = Corpus::empty();
    c.add(
        "knowledge/gateway.md",
        "---\nid: gateway\ntype: system\nname: Gateway\n---\n# Gateway\n",
    )
    .add(
        "knowledge/arch.md",
        "---\nid: arch\ntype: record\n---\n# Architecture\n\nSee ![wiring](../assets/wiring.puml).\n",
    )
    .add(
        "assets/wiring.puml",
        "@startuml\ncomponent \"API Gateway\" [[vaire/system:gateway]]\n@enduml\n",
    )
    .commit()
    .build();

    let refs = commands::refs::run(&c.ctx(), "record:arch", 1, None).unwrap();
    let edge = refs
        .refs
        .iter()
        .find(|r| r.id == "system:gateway")
        .expect("diagram edge to system:gateway");
    assert_eq!(edge.ref_type, "diagram");
    // The marker lives in the `.puml` file at line 2 — the edge's line points there,
    // not at the `arch.md` line that links to it.
    assert_eq!(edge.line, 2);
}

#[test]
fn dangling_diagram_reference_is_reported() {
    let c = Corpus::empty();
    c.add(
        "knowledge/arch.md",
        "---\nid: arch\ntype: record\n---\n# Architecture\n\n\
         ```mermaid\nclick A href \"vaire/system:missing\"\n```\n",
    )
    .commit()
    .build();

    let (report, _) = commands::check::run(&c.ctx(), false, false, false).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v, Violation::DanglingRef { to, .. } if to == "system:missing")),
        "expected a dangling_ref for the diagram edge: {:?}",
        report.violations
    );
}

#[test]
fn diagram_edge_alone_does_not_trigger_drift() {
    let c = Corpus::empty();
    c.add(
        "knowledge/gateway.md",
        "---\nid: gateway\ntype: system\nname: Gateway\n---\n# Gateway\n",
    )
    .add(
        "knowledge/arch.md",
        "---\nid: arch\ntype: record\n---\n# Architecture\n\n\
         ```plantuml\ncomponent A [[vaire/system:gateway]]\n```\n",
    )
    .commit()
    .build();

    let (report, _) = commands::check::run(&c.ctx(), false, false, false).unwrap();
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::Drift { .. })),
        "a diagram-only reference must not read as an unlisted inline reference: {:?}",
        report.warnings
    );
}

#[test]
fn inline_reference_still_drifts_even_with_a_matching_diagram_edge() {
    let c = Corpus::empty();
    c.add(
        "knowledge/gateway.md",
        "---\nid: gateway\ntype: system\nname: Gateway\n---\n# Gateway\n",
    )
    .add(
        "knowledge/arch.md",
        "---\nid: arch\ntype: record\n---\n# Architecture\n\n\
         Talks to [[system:gateway]].\n\n\
         ```plantuml\ncomponent A [[vaire/system:gateway]]\n```\n",
    )
    .commit()
    .build();

    let (report, _) = commands::check::run(&c.ctx(), false, false, false).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::Drift { to, .. } if to == "system:gateway")),
        "an inline reference not declared in frontmatter still drifts, diagram edge or not: {:?}",
        report.warnings
    );
}

#[test]
fn repeated_marker_across_fenced_blocks_is_one_edge() {
    let c = Corpus::empty();
    c.add(
        "knowledge/gateway.md",
        "---\nid: gateway\ntype: system\nname: Gateway\n---\n# Gateway\n",
    )
    .add(
        "knowledge/arch.md",
        "---\nid: arch\ntype: record\n---\n# Architecture\n\n\
         ```plantuml\ncomponent A [[vaire/system:gateway]]\n```\n\n\
         ```mermaid\nclick B href \"vaire/system:gateway\"\n```\n",
    )
    .commit()
    .build();

    let refs = commands::refs::run(&c.ctx(), "record:arch", 1, None).unwrap();
    let matches: Vec<_> = refs
        .refs
        .iter()
        .filter(|r| r.id == "system:gateway")
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "the same target marked twice must still be one edge: {:?}",
        refs.refs
    );
}

#[test]
fn incremental_reindex_picks_up_a_marker_newly_added_to_an_existing_diagram_file() {
    let c = Corpus::empty();
    c.add(
        "knowledge/gateway.md",
        "---\nid: gateway\ntype: system\nname: Gateway\n---\n# Gateway\n",
    )
    .add(
        "knowledge/arch.md",
        "---\nid: arch\ntype: record\n---\n# Architecture\n\nSee ![wiring](../assets/wiring.puml).\n",
    )
    // The diagram file starts out with no `vaire/` marker at all — no diagram edge, so
    // nothing in `edges` yet points a future invalidation check at `arch.md`.
    .add("assets/wiring.puml", "@startuml\ncomponent \"API Gateway\"\n@enduml\n")
    .commit();
    c.build_with(&DummyEmbedder { dims: 8 }, Mode::Full);

    let refs = commands::refs::run(&c.ctx(), "record:arch", 1, None).unwrap();
    assert!(
        !refs.refs.iter().any(|r| r.id == "system:gateway"),
        "no marker yet: {:?}",
        refs.refs
    );

    // Add the marker to the diagram file only — `arch.md` itself does not change.
    c.add(
        "assets/wiring.puml",
        "@startuml\ncomponent \"API Gateway\" [[vaire/system:gateway]]\n@enduml\n",
    )
    .commit();
    c.build_with(&DummyEmbedder { dims: 8 }, Mode::Incremental);

    let refs = commands::refs::run(&c.ctx(), "record:arch", 1, None).unwrap();
    assert!(
        refs.refs
            .iter()
            .any(|r| r.id == "system:gateway" && r.ref_type == "diagram"),
        "the marker added to an unchanged referencing node's linked diagram file must \
         still surface after an incremental reindex: {:?}",
        refs.refs
    );
}
