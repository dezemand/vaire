//! M5 PR-A: linked packages (`.vaire/packages/`), the cross-package resolver, and
//! cross-package `resolve`/`render` (design.md §9, cli.md §6.5). Runs against the
//! acceptance-criteria workspace: acme-core [team, person] ↔ acme-web [service] in a
//! dependency cycle, acme-shared [site] a leaf.

mod common;

use common::{Corpus, Ws};
use vaire::commands;
use vaire::error::VaireError;

// ---- vaire add --link ------------------------------------------------------

#[test]
fn add_link_creates_a_working_symlink() {
    let ws = Ws::new();
    ws.add_package("acme-core", &["team"], &[])
        .add_package("acme-web", &["service"], &[]);
    let out = commands::add::run(
        Some(&ws.root("acme-web")),
        None,
        "acme-core",
        Some(&ws.root("acme-core")),
    )
    .unwrap();
    assert_eq!(out.linked.as_deref(), Some("../../../acme-core"));

    let entry = ws.root("acme-web").join(".vaire/packages/acme-core");
    assert!(entry.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(entry.join("knowledge.toml").exists(), "link resolves");
}

#[test]
fn add_link_rejects_name_mismatch_and_missing_target() {
    let ws = Ws::new();
    ws.add_package("acme-core", &["team"], &[])
        .add_package("acme-web", &["service"], &[]);
    // Target declares acme-core, but we claim it is acme-shared.
    let err = commands::add::run(
        Some(&ws.root("acme-web")),
        None,
        "acme-shared",
        Some(&ws.root("acme-core")),
    )
    .unwrap_err();
    assert!(matches!(err, VaireError::Usage(_)), "{err}");
    // A bad --link must leave the manifest untouched.
    let manifest = std::fs::read_to_string(ws.root("acme-web").join("knowledge.toml")).unwrap();
    assert!(!manifest.contains("acme-shared"), "{manifest}");

    // Nonexistent target.
    let err = commands::add::run(
        Some(&ws.root("acme-web")),
        None,
        "acme-core",
        Some(&ws.root("no-such-dir")),
    )
    .unwrap_err();
    assert!(matches!(err, VaireError::Usage(_)), "{err}");
}

// ---- resolve (acceptance) --------------------------------------------------

#[test]
fn resolve_cross_package_finds_the_dep_file() {
    let ws = Ws::acceptance();
    let out = commands::resolve::run(&ws.ctx("acme-web"), "@acme-core/team:platform").unwrap();
    assert_eq!(out.id, "@acme-core/team:platform");
    assert_eq!(out.package.as_deref(), Some("acme-core"));
    assert_eq!(out.path, "knowledge/platform.md");
    assert_eq!(out.frontmatter["name"], "Platform Team");
}

#[test]
fn resolve_undeclared_package_errors_with_dependency_error() {
    let ws = Ws::acceptance();
    let err = commands::resolve::run(&ws.ctx("acme-web"), "@acme-nope/team:x").unwrap_err();
    assert!(matches!(err, VaireError::Dependency(_)), "{err}");
    assert!(
        err.to_string().contains("not a declared dependency"),
        "{err}"
    );
}

#[test]
fn resolve_declared_but_unlinked_names_the_fix() {
    let ws = Ws::new();
    ws.add_package("solo", &["note"], &[("acme-core", "^1")])
        .add_file(
            "solo",
            "knowledge/n.md",
            "---\nid: n\ntype: note\n---\n# N\n",
        )
        .commit("solo")
        .build("solo");
    let err = commands::resolve::run(&ws.ctx("solo"), "@acme-core/team:x").unwrap_err();
    assert!(matches!(err, VaireError::Dependency(_)), "{err}");
    assert!(err.to_string().contains("--link"), "fix named: {err}");
}

#[test]
fn resolve_missing_node_in_dep_is_id_not_found() {
    let ws = Ws::acceptance();
    let err = commands::resolve::run(&ws.ctx("acme-web"), "@acme-core/team:nope").unwrap_err();
    assert!(matches!(err, VaireError::IdNotFound(_)), "{err}");
    assert!(err.to_string().contains("@acme-core/team:nope"), "{err}");
}

// ---- superseded_by across packages ----------------------------------------

#[test]
fn cross_package_tombstone_rebinds_to_owner() {
    let ws = Ws::acceptance();
    // service:legacy (acme-web) is superseded by @acme-core/team:platform.
    let out = commands::resolve::run(&ws.ctx("acme-web"), "service:legacy").unwrap();
    assert_eq!(out.id, "@acme-core/team:platform");
    assert_eq!(out.package.as_deref(), Some("acme-core"));
    assert_eq!(out.requested_id.as_deref(), Some("service:legacy"));
    assert_eq!(
        out.superseded_by.as_deref(),
        Some("@acme-core/team:platform")
    );
}

#[test]
fn cross_package_supersession_cycle_terminates_and_returns() {
    // a (in one) → b (in two) → a: same stop-and-return semantics as local redirects.
    let ws = Ws::new();
    ws.add_package("one", &["team"], &[("two", "^1")])
        .add_file(
            "one",
            "knowledge/a.md",
            "---\nid: a\ntype: team\nsuperseded_by: \"@two/team:b\"\n---\n# A\n",
        )
        .commit("one");
    ws.add_package("two", &["team"], &[("one", "^1")])
        .add_file(
            "two",
            "knowledge/b.md",
            "---\nid: b\ntype: team\nsuperseded_by: \"@one/team:a\"\n---\n# B\n",
        )
        .commit("two");
    ws.link("one", "two").link("two", "one");
    ws.build("one").build("two");

    let out = commands::resolve::run(&ws.ctx("one"), "team:a").unwrap();
    assert_eq!(out.id, "team:a", "cycle stops and returns the closing node");
}

// ---- the dependency cycle + run-root fallback ------------------------------

#[test]
fn dep_edge_back_into_run_root_resolves_without_a_link() {
    // acme-core links nothing: resolving its flagship (@acme-web/service:checkout) from
    // acme-web works because the run-root itself needs no link.
    let ws = Ws::acceptance();
    let rendered = commands::render::run(&ws.ctx("acme-web"), "@acme-core/team:platform").unwrap();
    assert_eq!(rendered.package.as_deref(), Some("acme-core"));
}

// ---- render ----------------------------------------------------------------

#[test]
fn render_cross_package_hrefs_go_through_the_workspace() {
    let ws = Ws::acceptance();
    let out = commands::render::run(&ws.ctx("acme-web"), "service:checkout").unwrap();
    assert!(
        out.markdown
            .contains("[Platform Team](../../acme-core/knowledge/platform.md)"),
        "{}",
        out.markdown
    );
    assert!(
        out.markdown
            .contains("[HQ](../../acme-shared/knowledge/hq.md)"),
        "piped display wins: {}",
        out.markdown
    );
}

#[test]
fn render_local_output_is_unchanged() {
    // Same-package hrefs keep the exact pre-M5 shape (./ prefix and all).
    let c = Corpus::empty();
    c.add(
        "knowledge/a.md",
        "---\nid: a\ntype: method\nname: Method A\n---\n# A\n\nSee [[method:b]].\n",
    )
    .add(
        "knowledge/b.md",
        "---\nid: b\ntype: method\nname: Method B\n---\n# B\n",
    )
    .commit()
    .build();
    let out = commands::render::run(&c.ctx(), "method:a").unwrap();
    assert!(
        out.markdown.contains("[Method B](./b.md)"),
        "{}",
        out.markdown
    );
    assert!(out.package.is_none());
}

// ---- federation invariants --------------------------------------------------

#[test]
fn colliding_ids_and_paths_never_interfere_across_packages() {
    // Two members both define person:jane at knowledge/x.md. Deleting one member's file
    // must not touch the other member's rows — nothing merges, so nothing collides.
    let ws = Ws::acceptance();
    ws.add_file(
        "acme-core",
        "knowledge/x.md",
        "---\nid: jane\ntype: person\nname: Core Jane\n---\n# Core Jane\n",
    )
    .commit("acme-core");
    ws.add_file(
        "acme-shared",
        "knowledge/x.md",
        "---\nid: jane\ntype: person\nname: Shared Jane\n---\n# Shared Jane\n",
    )
    .commit("acme-shared");
    ws.build("acme-core").build("acme-shared");

    // Both resolve, each in its own package.
    let core = commands::resolve::run(&ws.ctx("acme-web"), "@acme-core/person:jane").unwrap();
    assert_eq!(core.frontmatter["name"], "Core Jane");
    let shared = commands::resolve::run(&ws.ctx("acme-web"), "@acme-shared/person:jane").unwrap();
    assert_eq!(shared.frontmatter["name"], "Shared Jane");

    // Delete acme-shared's file; acme-core's node survives untouched.
    std::fs::remove_file(ws.root("acme-shared").join("knowledge/x.md")).unwrap();
    ws.commit("acme-shared").build("acme-shared");
    let core = commands::resolve::run(&ws.ctx("acme-web"), "@acme-core/person:jane").unwrap();
    assert_eq!(core.frontmatter["name"], "Core Jane");
    assert!(
        commands::resolve::run(&ws.ctx("acme-web"), "@acme-shared/person:jane").is_err(),
        "the deleted one is gone"
    );
}

#[test]
fn standalone_package_never_touches_packages_dir() {
    let c = Corpus::fixture();
    assert!(commands::resolve::run(&c.ctx(), "person:jane-doe").is_ok());
    assert!(
        !c.packages_dir().exists(),
        "no .vaire/packages appears for a lone package"
    );
}

// ---- vaire index ensure pass ------------------------------------------------

#[test]
fn index_builds_linked_dependencies_and_snapshots() {
    let ws = Ws::acceptance();
    // Wipe the dep indexes so the ensure pass has real work.
    for pkg in ["acme-core", "acme-shared"] {
        let _ = std::fs::remove_file(ws.root(pkg).join(".vaire/index.db"));
    }

    let out = commands::index::run(&ws.ctx("acme-web"), false, false, false, false).unwrap();
    let statuses: Vec<(String, String)> = out
        .dependencies
        .iter()
        .map(|d| (d.name.clone(), d.status.clone()))
        .collect();
    assert!(
        statuses
            .iter()
            .any(|(n, s)| n == "acme-core" && s == "indexed"),
        "{statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .any(|(n, s)| n == "acme-shared" && s == "indexed"),
        "{statuses:?}"
    );
    assert!(ws.root("acme-core").join(".vaire/index.db").exists());
    assert!(ws.root("acme-shared").join(".vaire/index.db").exists());

    // The consumer recorded its resolution snapshot (the lockfile precursor).
    let index = vaire::index::Index::open(&ws.root("acme-web").join(".vaire/index.db")).unwrap();
    let snapshot = index.meta("deps_snapshot").unwrap().expect("snapshot set");
    assert!(snapshot.contains("\"acme-core\""), "{snapshot}");
    assert!(snapshot.contains("\"acme-shared\""), "{snapshot}");
}

#[test]
fn index_warns_but_continues_on_unlinked_dependency() {
    let ws = Ws::new();
    ws.add_package("solo", &["note"], &[("acme-core", "^1")])
        .add_file(
            "solo",
            "knowledge/n.md",
            "---\nid: n\ntype: note\n---\n# N\n",
        )
        .commit("solo");

    let out = commands::index::run(&ws.ctx("solo"), false, false, false, false).unwrap();
    assert_eq!(out.summary.nodes, 1, "own package indexed fine");
    assert_eq!(out.dependencies.len(), 1);
    assert_eq!(out.dependencies[0].status, "missing");
    assert!(
        out.dependencies[0]
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("--link"),
        "fix named: {:?}",
        out.dependencies[0].note
    );
}
