//! Spec tests for project-scoped record IDs — path style `<project-id>/<type>:<local>`
//! (cli.md §6.1).

mod common;

use common::{Corpus, DummyEmbedder};
use vaire::commands;
use vaire::config::Config;
use vaire::index::build::Mode;

fn scoped() -> Config {
    Config {
        scoped_types: vec!["record".to_string()],
        ..Config::default()
    }
}

/// Two projects, each owning a record with the same *local* id, plus a person.
fn two_project_corpus() -> Corpus {
    let c = Corpus::empty();
    // Write the config so `ctx`-based commands (render) see scoped_types too.
    c.add(".vaire/config.toml", "scoped_types = [\"record\"]\n")
    .add(
        "knowledge/people/jane.md",
        "---\nid: jane\ntype: person\nname: Jane\n---\n# Jane\n",
    )
    .add(
        "projects/atlas/README.md",
        "---\nid: atlas-2026-q2\ntype: project\nname: Atlas\n---\n# Atlas\n",
    )
    .add(
        "projects/beta/README.md",
        "---\nid: beta-2026\ntype: project\nname: Beta\n---\n# Beta\n",
    )
    .add(
        "projects/atlas/standup.md",
        // local id `standup`; full cross-type ref + a relative sibling ref
        "---\nid: standup\ntype: record\nscope: project:atlas-2026-q2\nparticipants: [person:jane]\n---\n# Atlas standup\n\nFollows [[record:kickoff]] with [[person:jane]].\n",
    )
    .add(
        "projects/atlas/kickoff.md",
        "---\nid: kickoff\ntype: record\nscope: project:atlas-2026-q2\n---\n# Atlas kickoff\n",
    )
    .add(
        "projects/beta/standup.md",
        "---\nid: standup\ntype: record\nscope: project:beta-2026\n---\n# Beta standup\n",
    )
    .commit();
    c.build_cfg(&scoped(), &DummyEmbedder { dims: 8 }, Mode::Full);
    c
}

#[test]
fn local_id_composes_under_the_project_id() {
    let c = two_project_corpus();
    let out = commands::resolve::run(&c.ctx(), "project:atlas-2026-q2/record:standup").unwrap();
    assert_eq!(out.id, "project:atlas-2026-q2/record:standup");
    assert_eq!(out.node_type, "record"); // the node's own type is the last segment
    assert_eq!(out.path, "projects/atlas/standup.md");
}

#[test]
fn same_local_id_in_two_projects_does_not_collide() {
    let c = two_project_corpus();
    assert_eq!(
        commands::resolve::run(&c.ctx(), "project:atlas-2026-q2/record:standup")
            .unwrap()
            .path,
        "projects/atlas/standup.md"
    );
    assert_eq!(
        commands::resolve::run(&c.ctx(), "project:beta-2026/record:standup")
            .unwrap()
            .path,
        "projects/beta/standup.md"
    );
}

#[test]
fn full_reference_resolves_to_scoped_node() {
    let c = two_project_corpus();
    let out = commands::backlinks::run(&c.ctx(), "person:jane", None, None).unwrap();
    assert!(
        out.backlinks
            .iter()
            .any(|b| b.id == "project:atlas-2026-q2/record:standup")
    );
}

#[test]
fn relative_reference_expands_to_own_project() {
    let c = two_project_corpus();
    let out =
        commands::refs::run(&c.ctx(), "project:atlas-2026-q2/record:standup", 1, None).unwrap();
    // [[record:kickoff]] inside atlas's standup → project:atlas-2026-q2/record:kickoff.
    assert!(
        out.refs
            .iter()
            .any(|r| r.id == "project:atlas-2026-q2/record:kickoff")
    );
    // …and not the bare/unscoped or other-project form.
    assert!(!out.refs.iter().any(|r| r.id == "record:kickoff"));
    assert!(
        !out.refs
            .iter()
            .any(|r| r.id == "project:beta-2026/record:kickoff")
    );
    // The project: edge itself stays an unscoped reference to the project entity.
    assert!(out.refs.iter().any(|r| r.id == "project:atlas-2026-q2"));
}

#[test]
fn search_shows_full_ids_unscoped_and_local_ids_under_scope() {
    let c = two_project_corpus();

    // No --scope: scoped results carry their full `<scope>/type:id`.
    let unscoped = commands::search::run(&c.ctx(), "standup", None, None, Some(10)).unwrap();
    assert!(
        unscoped
            .results
            .iter()
            .any(|r| r.id == "project:atlas-2026-q2/record:standup")
    );

    // With --scope: the prefix is implied, so results show the local `type:id`.
    let scoped = commands::search::run(
        &c.ctx(),
        "standup",
        None,
        Some("project:atlas-2026-q2"),
        Some(10),
    )
    .unwrap();
    assert!(scoped.results.iter().any(|r| r.id == "record:standup"));
    assert!(scoped.results.iter().all(|r| !r.id.contains('/')));
}

#[test]
fn render_resolves_relative_scoped_links() {
    let c = two_project_corpus();
    let md = commands::render::run(&c.ctx(), "project:atlas-2026-q2/record:standup")
        .unwrap()
        .markdown;
    // The relative [[record:kickoff]] renders as a resolved link to the sibling file.
    assert!(md.contains("](./kickoff.md)"), "got:\n{md}");
}

#[test]
fn scope_field_is_configurable() {
    // Scope under an `area:` container instead of the default `project:`.
    let c = Corpus::empty();
    c.add(
        ".vaire/config.toml",
        "scoped_types = [\"record\"]\nscope_field = \"area\"\n",
    )
    .add(
        "knowledge/platform.md",
        "---\nid: platform\ntype: area\nname: Platform\n---\n# Platform\n",
    )
    .add(
        "notes/note.md",
        "---\nid: note\ntype: record\narea: area:platform\n---\n# Note\n",
    )
    .commit();
    let cfg = Config {
        scoped_types: vec!["record".to_string()],
        scope_field: "area".to_string(),
        include: vec!["knowledge/**/*.md".into(), "notes/**/*.md".into()],
        ..Config::default()
    };
    c.build_cfg(&cfg, &DummyEmbedder { dims: 8 }, Mode::Full);

    let out = commands::resolve::run(&c.ctx(), "area:platform/record:note").unwrap();
    assert_eq!(out.id, "area:platform/record:note");
    assert_eq!(out.path, "notes/note.md");
}

#[test]
fn scoping_is_off_by_default() {
    let c = Corpus::empty();
    c.add(
        "projects/atlas/README.md",
        "---\nid: atlas\ntype: project\nname: Atlas\n---\n# Atlas\n",
    )
    .add(
        "projects/atlas/standup.md",
        "---\nid: standup\ntype: record\nscope: project:atlas\n---\n# Standup\n",
    )
    .commit()
    .build(); // default config: scoped_types empty

    assert_eq!(
        commands::resolve::run(&c.ctx(), "record:standup")
            .unwrap()
            .path,
        "projects/atlas/standup.md"
    );
    assert_eq!(
        commands::resolve::run(&c.ctx(), "project:atlas/record:standup")
            .unwrap_err()
            .exit_code(),
        vaire::error::ExitCode::IdNotFound
    );
}
