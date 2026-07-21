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
    // A dep that never ran `vaire init` still gets the derived-dir gitignore, so the
    // consumer's ensure pass never litters the dep's repo with untracked noise.
    assert!(ws.root("acme-core").join(".vaire/.gitignore").exists());

    // The consumer recorded its resolution snapshot (the lockfile precursor), including
    // its own constraint for direct dependencies.
    let index = vaire::index::Index::open(&ws.root("acme-web").join(".vaire/index.db")).unwrap();
    let snapshot = index.meta("deps_snapshot").unwrap().expect("snapshot set");
    assert!(snapshot.contains("\"acme-core\""), "{snapshot}");
    assert!(snapshot.contains("\"acme-shared\""), "{snapshot}");
    assert!(snapshot.contains("\"constraint\":\"^1\""), "{snapshot}");
}

// ---- backlinks / refs across packages (M5b) ---------------------------------

#[test]
fn backlinks_of_a_dep_node_finds_referencing_services_here() {
    // Acceptance: from acme-web, `backlinks @acme-core/team:platform` → the acme-web
    // services that reference it (frontmatter owner edge + the inline mention).
    let ws = Ws::acceptance();
    let out = commands::backlinks::run(&ws.ctx("acme-web"), "@acme-core/team:platform", None, None)
        .unwrap();
    let ids: Vec<&str> = out.backlinks.iter().map(|b| b.id.as_str()).collect();
    assert!(ids.contains(&"service:checkout"), "{ids:?}");
    assert!(
        out.backlinks
            .iter()
            .all(|b| b.package.is_none() && b.path.starts_with("knowledge/")),
        "local referencers, package-relative paths: {ids:?}"
    );
    assert!(out.skipped.is_empty());
}

#[test]
fn backlinks_of_a_local_node_includes_cross_package_referencers() {
    // The other direction: who references MY service:checkout? acme-core's platform
    // team does, via its flagship edge — a cross-package inbound row.
    let ws = Ws::acceptance();
    let out =
        commands::backlinks::run(&ws.ctx("acme-web"), "service:checkout", None, None).unwrap();
    let row = out
        .backlinks
        .iter()
        .find(|b| b.id == "@acme-core/team:platform")
        .unwrap_or_else(|| panic!("cross-package inbound expected: {:?}", out.backlinks));
    assert_eq!(row.package.as_deref(), Some("acme-core"));
    assert_eq!(row.path, "knowledge/platform.md");
}

#[test]
fn refs_shows_cross_package_edges_and_depth_two_comes_back() {
    // Acceptance: `refs service:checkout` → its @acme-core / @acme-shared edges.
    let ws = Ws::acceptance();
    let out = commands::refs::run(&ws.ctx("acme-web"), "service:checkout", 1, None).unwrap();
    let ids: Vec<&str> = out.refs.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"@acme-core/team:platform"), "{ids:?}");
    assert!(ids.contains(&"@acme-shared/site:hq"), "{ids:?}");

    // Depth 2: platform's edges (jane locally, checkout back HERE) — the cycle
    // terminates and the start node never reappears.
    let out = commands::refs::run(&ws.ctx("acme-web"), "service:checkout", 2, None).unwrap();
    let ids: Vec<&str> = out.refs.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"@acme-core/person:jane-doe"), "{ids:?}");
    assert!(
        !ids.contains(&"service:checkout"),
        "start node deduped: {ids:?}"
    );
    let jane = out
        .refs
        .iter()
        .find(|r| r.id == "@acme-core/person:jane-doe")
        .unwrap();
    assert_eq!(jane.distance, Some(2));
}

#[test]
fn refs_skips_and_surfaces_an_unavailable_dependency() {
    // Break acme-shared's link: its edges drop like dangling refs, but the package is
    // named in `skipped` rather than silently vanishing.
    let ws = Ws::acceptance();
    std::fs::remove_dir_all(ws.root("acme-shared")).unwrap();
    let out = commands::refs::run(&ws.ctx("acme-web"), "service:checkout", 1, None).unwrap();
    let ids: Vec<&str> = out.refs.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"@acme-core/team:platform"), "{ids:?}");
    assert!(!ids.iter().any(|i| i.contains("acme-shared")), "{ids:?}");
    assert!(
        out.skipped.contains(&"acme-shared".to_string()),
        "{:?}",
        out.skipped
    );
}

#[test]
fn backlinks_limit_applies_after_the_merge() {
    // LIMIT push-down: per-member caps, then a global re-limit — the final count is
    // exact even when hits span members.
    let ws = Ws::acceptance();
    let all =
        commands::backlinks::run(&ws.ctx("acme-web"), "service:checkout", None, None).unwrap();
    assert!(
        all.count >= 2,
        "fixture has local+cross inbound: {}",
        all.count
    );
    let one =
        commands::backlinks::run(&ws.ctx("acme-web"), "service:checkout", None, Some(1)).unwrap();
    assert_eq!(one.count, 1);
    // Deterministic: the first row of the unlimited result.
    assert_eq!(one.backlinks[0].id, all.backlinks[0].id);
}

// ---- search / suggest across packages (M5b) ---------------------------------

#[test]
fn search_covers_the_dependency_closure() {
    // "What you depend on is part of your knowledge": a query from acme-web finds
    // acme-core's platform team, qualified and package-tagged.
    let ws = Ws::acceptance();
    let out = commands::search::run(&ws.ctx("acme-web"), "platform", None, None, Some(10), false)
        .unwrap();
    let hit = out
        .results
        .iter()
        .find(|r| r.id == "@acme-core/team:platform")
        .unwrap_or_else(|| {
            panic!(
                "dep hit expected: {:?}",
                out.results.iter().map(|r| &r.id).collect::<Vec<_>>()
            )
        });
    assert_eq!(hit.package.as_deref(), Some("acme-core"));
    assert_eq!(hit.path, "knowledge/platform.md");
    assert!(out.skipped.is_empty());
}

#[test]
fn search_local_restricts_to_this_package() {
    let ws = Ws::acceptance();
    let out =
        commands::search::run(&ws.ctx("acme-web"), "platform", None, None, Some(10), true).unwrap();
    assert!(
        out.results.iter().all(|r| r.package.is_none()),
        "{:?}",
        out.results.iter().map(|r| &r.id).collect::<Vec<_>>()
    );
}

#[test]
fn search_ranking_is_deterministic_across_members() {
    // Same query twice → identical ordering (score desc, qualified id asc ties).
    let ws = Ws::acceptance();
    let ids = |o: &vaire::output::SearchOutput| {
        o.results.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
    };
    let a = commands::search::run(
        &ws.ctx("acme-web"),
        "checkout team",
        None,
        None,
        Some(10),
        false,
    )
    .unwrap();
    let b = commands::search::run(
        &ws.ctx("acme-web"),
        "checkout team",
        None,
        None,
        Some(10),
        false,
    )
    .unwrap();
    assert_eq!(ids(&a), ids(&b));
    assert!(!a.results.is_empty());
}

#[test]
fn search_skips_and_surfaces_unavailable_dependency() {
    let ws = Ws::acceptance();
    std::fs::remove_file(ws.root("acme-shared").join(".vaire/index.db")).unwrap();
    let out = commands::search::run(
        &ws.ctx("acme-web"),
        "headquarters",
        None,
        None,
        Some(10),
        false,
    )
    .unwrap();
    assert!(
        out.skipped.contains(&"acme-shared".to_string()),
        "{:?}",
        out.skipped
    );
    assert!(
        !out.results.iter().any(|r| r.id.contains("acme-shared")),
        "no hits from the skipped member"
    );
}

#[test]
fn suggest_returns_pre_qualified_dep_ids() {
    // The lookup-before-reference flow across packages: the suggestion is ready to
    // paste as [[@acme-core/person:jane-doe]].
    let ws = Ws::acceptance();
    let out =
        commands::suggest::run(&ws.ctx("acme-web"), "jane doe", None, Some(5), false).unwrap();
    let hit = out
        .suggestions
        .iter()
        .find(|s| s.id == "@acme-core/person:jane-doe")
        .unwrap_or_else(|| {
            panic!(
                "{:?}",
                out.suggestions.iter().map(|s| &s.id).collect::<Vec<_>>()
            )
        });
    assert_eq!(hit.package.as_deref(), Some("acme-core"));

    // --local: dep suggestions disappear.
    let local =
        commands::suggest::run(&ws.ctx("acme-web"), "jane doe", None, Some(5), true).unwrap();
    assert!(local.suggestions.iter().all(|s| s.package.is_none()));
}

// ---- unresolved / status across packages (M5b) ------------------------------

#[test]
fn unresolved_defaults_to_this_packages_worklist() {
    // A dependency's loose ends are its owner's worklist (packages.md §7).
    let ws = Ws::acceptance();
    let out = commands::unresolved::run(&ws.ctx("acme-web"), None, None, false).unwrap();
    let descs: Vec<&str> = out
        .unresolved
        .iter()
        .map(|u| u.descriptor.as_str())
        .collect();
    assert!(descs.contains(&"the on-call lead"), "{descs:?}");
    assert!(!descs.contains(&"the incident manager"), "{descs:?}");
    assert!(out.unresolved.iter().all(|u| u.package.is_none()));
}

#[test]
fn unresolved_all_packages_tags_dependency_rows() {
    let ws = Ws::acceptance();
    let out = commands::unresolved::run(&ws.ctx("acme-web"), None, None, true).unwrap();
    let dep_row = out
        .unresolved
        .iter()
        .find(|u| u.descriptor == "the incident manager")
        .unwrap_or_else(|| panic!("{:?}", out.unresolved));
    assert_eq!(dep_row.package.as_deref(), Some("acme-core"));
    assert_eq!(dep_row.record, "@acme-core/team:platform");
    // The run-root's own row stays untagged.
    assert!(
        out.unresolved
            .iter()
            .any(|u| u.descriptor == "the on-call lead" && u.package.is_none())
    );
}

#[test]
fn unresolved_scope_and_all_packages_conflict() {
    let ws = Ws::acceptance();
    let err =
        commands::unresolved::run(&ws.ctx("acme-web"), None, Some("project:x"), true).unwrap_err();
    assert!(matches!(err, VaireError::Usage(_)), "{err}");
}

#[test]
fn status_reports_each_dependency_state() {
    let ws = Ws::acceptance();
    let out = commands::status::run(&ws.ctx("acme-web")).unwrap();
    assert_eq!(
        out.embed_provider.as_deref(),
        Some("unknown:8"),
        "dummy identity"
    );

    let core = out
        .dependencies
        .iter()
        .find(|d| d.name == "acme-core")
        .unwrap();
    assert!(core.linked);
    assert_eq!(core.index, "fresh");
    assert!(core.nodes > 0);
    assert_eq!(core.version.as_deref(), Some("1.0.0"));
    assert_eq!(core.commits_behind_head, 0);

    // Stale schema is reported, not fatal.
    let shared_db = ws.root("acme-shared").join(".vaire/index.db");
    let index = vaire::index::Index::open(&shared_db).unwrap();
    index.set_schema_version(999).unwrap();
    drop(index);
    let out = commands::status::run(&ws.ctx("acme-web")).unwrap();
    let shared = out
        .dependencies
        .iter()
        .find(|d| d.name == "acme-shared")
        .unwrap();
    assert_eq!(shared.index, "stale-schema");

    // An unlinked dependency is a reported row with the fix, never a failure.
    std::fs::remove_file(ws.root("acme-web").join(".vaire/packages/acme-shared")).unwrap();
    let out = commands::status::run(&ws.ctx("acme-web")).unwrap();
    let shared = out
        .dependencies
        .iter()
        .find(|d| d.name == "acme-shared")
        .unwrap();
    assert!(!shared.linked);
    assert!(
        shared
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("--link"),
        "{:?}",
        shared.note
    );
}

#[test]
fn empty_results_still_surface_skipped_dependencies() {
    // "Surfaced, never silently dropped" holds even when a fan-out read finds nothing.
    let ws = Ws::acceptance();
    std::fs::remove_file(ws.root("acme-shared").join(".vaire/index.db")).unwrap();
    let out = commands::search::run(
        &ws.ctx("acme-web"),
        "zzqxnomatchqq",
        None,
        None,
        Some(10),
        false,
    )
    .unwrap();
    assert!(out.results.is_empty());
    assert!(out.skipped.contains(&"acme-shared".to_string()));
    assert!(
        vaire::output::Output::render_human(&out).contains("acme-shared"),
        "human empty output keeps the skipped note"
    );
}

// ---- per-consumer link precedence (the reason for the npm model) ------------

/// The diamond: acme-app (run-root) depends on acme-mid and acme-shared; TWO directories
/// (`shared-v1`, `shared-v2`) both declare `name = "acme-shared"`. acme-mid's `thing:m`
/// is superseded by `@acme-shared/site:hq`, so resolving `@acme-mid/thing:m` from
/// acme-app exercises WHOSE link answers "acme-shared" for acme-mid. acme-app always
/// links acme-mid and links acme-shared → shared-v2.
fn diamond(mid_links_v1: bool, mid_declares: bool) -> Ws {
    let ws = Ws::new();
    let mid_deps: &[(&str, &str)] = if mid_declares {
        &[("acme-shared", "^1")]
    } else {
        &[]
    };
    ws.add_package("acme-mid", &["thing"], mid_deps)
        .add_file(
            "acme-mid",
            "knowledge/m.md",
            "---\nid: m\ntype: thing\nsuperseded_by: \"@acme-shared/site:hq\"\n---\n# M\n",
        )
        .commit("acme-mid");
    for (dir, label) in [("shared-v1", "HQ v1"), ("shared-v2", "HQ v2")] {
        ws.add_package_named(dir, "acme-shared", &["site"], &[])
            .add_file(
                dir,
                "knowledge/hq.md",
                &format!("---\nid: hq\ntype: site\nname: {label}\n---\n# {label}\n"),
            )
            .commit(dir);
    }
    ws.add_package(
        "acme-app",
        &["app"],
        &[("acme-mid", "^1"), ("acme-shared", "^1")],
    )
    .add_file(
        "acme-app",
        "knowledge/a.md",
        "---\nid: a\ntype: app\nname: App\n---\n# App\n",
    )
    .commit("acme-app");

    ws.link("acme-app", "acme-mid");
    ws.link_to("acme-app", "acme-shared", "shared-v2");
    if mid_links_v1 {
        ws.link_to("acme-mid", "acme-shared", "shared-v1");
    }
    ws.build("acme-mid")
        .build("shared-v1")
        .build("shared-v2")
        .build("acme-app");
    ws
}

#[test]
fn own_link_wins_over_run_roots_for_the_same_name() {
    // acme-mid's own link (→ v1) answers ITS reference; acme-app's link (→ v2) answers
    // acme-app's own — one name, two targets, per-consumer mapping.
    let ws = diamond(true, true);
    let via_mid = commands::resolve::run(&ws.ctx("acme-app"), "@acme-mid/thing:m").unwrap();
    assert_eq!(via_mid.frontmatter["name"], "HQ v1");
    let direct = commands::resolve::run(&ws.ctx("acme-app"), "@acme-shared/site:hq").unwrap();
    assert_eq!(direct.frontmatter["name"], "HQ v2");
}

#[test]
fn run_root_links_serve_transitive_deps_as_fallback() {
    // acme-mid declares acme-shared but links nothing → the run-root's flat link answers.
    let ws = diamond(false, true);
    let via_mid = commands::resolve::run(&ws.ctx("acme-app"), "@acme-mid/thing:m").unwrap();
    assert_eq!(via_mid.frontmatter["name"], "HQ v2");
}

#[test]
fn transitive_reference_requires_the_source_packages_declaration() {
    // acme-mid does NOT declare acme-shared: its reference must fail even though the
    // run-root has a perfectly good link — resolution is keyed by the SOURCE manifest.
    let ws = diamond(false, false);
    let err = commands::resolve::run(&ws.ctx("acme-app"), "@acme-mid/thing:m").unwrap_err();
    assert!(matches!(err, VaireError::Dependency(_)), "{err}");
    assert!(
        err.to_string()
            .contains("not a declared dependency of 'acme-mid'"),
        "{err}"
    );
}

// ---- link lifecycle ---------------------------------------------------------

#[test]
fn add_link_replaces_a_symlink_but_never_a_real_directory() {
    let ws = Ws::new();
    ws.add_package("acme-core", &["team"], &[])
        .add_package_named("core-fork", "acme-core", &["team"], &[])
        .add_package("acme-web", &["service"], &[]);

    // First link → acme-core dir; re-link → replaced by core-fork.
    ws.link("acme-web", "acme-core");
    ws.link_to("acme-web", "acme-core", "core-fork");
    let entry = ws.root("acme-web").join(".vaire/packages/acme-core");
    assert!(
        std::fs::canonicalize(&entry)
            .unwrap()
            .ends_with("core-fork"),
        "second --link replaces the symlink"
    );

    // A REAL directory at the entry is never touched (it may be installed content) —
    // and the refusal happens BEFORE the manifest is written (§4.2a: a bad --link
    // leaves everything unchanged).
    std::fs::remove_file(&entry).unwrap();
    std::fs::create_dir_all(&entry).unwrap();
    let manifest_before =
        std::fs::read_to_string(ws.root("acme-web").join("knowledge.toml")).unwrap();
    let err = commands::add::run(
        Some(&ws.root("acme-web")),
        None,
        "acme-core@^9",
        Some(&ws.root("acme-core")),
    )
    .unwrap_err();
    assert!(matches!(err, VaireError::Usage(_)), "{err}");
    assert!(err.to_string().contains("refusing"), "{err}");
    let manifest_after =
        std::fs::read_to_string(ws.root("acme-web").join("knowledge.toml")).unwrap();
    assert_eq!(manifest_before, manifest_after, "manifest untouched");
}

#[test]
fn add_rejects_a_self_dependency_before_writing() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[]);
    let manifest_before =
        std::fs::read_to_string(ws.root("acme-web").join("knowledge.toml")).unwrap();
    let err = commands::add::run(Some(&ws.root("acme-web")), None, "acme-web", None).unwrap_err();
    assert!(matches!(err, VaireError::Usage(_)), "{err}");
    assert!(err.to_string().contains("cannot depend on itself"), "{err}");
    let manifest_after =
        std::fs::read_to_string(ws.root("acme-web").join("knowledge.toml")).unwrap();
    assert_eq!(manifest_before, manifest_after, "manifest untouched");
}

#[test]
fn broken_link_is_its_own_failure_class() {
    let ws = Ws::acceptance();
    std::fs::remove_dir_all(ws.root("acme-shared")).unwrap();
    let err = commands::resolve::run(&ws.ctx("acme-web"), "@acme-shared/site:hq").unwrap_err();
    assert!(matches!(err, VaireError::Dependency(_)), "{err}");
    assert!(err.to_string().contains("broken link"), "{err}");
}

#[test]
fn renamed_target_fails_the_declared_identity_check() {
    // The link resolves, but the target no longer declares the expected name.
    let ws = Ws::acceptance();
    std::fs::write(
        ws.root("acme-shared").join("knowledge.toml"),
        "name = \"acme-other\"\nversion = \"1.0.0\"\ntypes = [\"site\"]\n",
    )
    .unwrap();
    let err = commands::resolve::run(&ws.ctx("acme-web"), "@acme-shared/site:hq").unwrap_err();
    assert!(matches!(err, VaireError::Dependency(_)), "{err}");
    assert!(err.to_string().contains("declares name"), "{err}");
}

// ---- render: the dep file's own context ------------------------------------

#[test]
fn rendered_dep_file_keeps_local_links_package_internal() {
    // Rendering @acme-core/team:platform from acme-web: its LOCAL ref stays a
    // package-internal relative link (portable markdown), while its ref back to the
    // run-root crosses the workspace.
    let ws = Ws::acceptance();
    let out = commands::render::run(&ws.ctx("acme-web"), "@acme-core/team:platform").unwrap();
    assert!(
        out.markdown.contains("[Jane Doe](./jane.md)"),
        "local ref of the dep file: {}",
        out.markdown
    );
    assert!(
        out.markdown
            .contains("[Checkout](../../acme-web/knowledge/checkout.md)"),
        "ref back into the run-root: {}",
        out.markdown
    );
}

// ---- symmetry: a dep checkout is its own run-root ---------------------------

#[test]
fn a_dep_checkout_works_standalone_as_its_own_run_root() {
    let ws = Ws::acceptance();
    let out = commands::resolve::run(&ws.ctx("acme-core"), "person:jane-doe").unwrap();
    assert_eq!(out.id, "person:jane-doe");
    assert!(out.package.is_none(), "bare and local from its own root");
}

// ---- ensure pass details ----------------------------------------------------

#[test]
fn provider_switch_rebuilds_dep_vectors_homogeneously() {
    // Fixture deps were built with the dummy embedder (identity "unknown:8"); the ensure
    // pass runs the configured provider (local:*) → full rebuild, homogeneous vectors,
    // recorded identity. A second run leaves it alone (incremental).
    let ws = Ws::acceptance();
    commands::index::run(&ws.ctx("acme-web"), false, false, false, false).unwrap();

    let meta = |pkg: &str| {
        let index = vaire::index::Index::open(&ws.root(pkg).join(".vaire/index.db")).unwrap();
        index.meta("embed_provider").unwrap().unwrap_or_default()
    };
    let after_first = meta("acme-core");
    assert!(
        after_first.starts_with("local:"),
        "dep re-embedded by the configured provider: {after_first}"
    );

    let out = commands::index::run(&ws.ctx("acme-web"), false, false, false, false).unwrap();
    assert_eq!(meta("acme-core"), after_first, "second run is a no-op");
    assert!(
        out.dependencies.iter().all(|d| d.status == "indexed"),
        "{:?}",
        out.dependencies
    );
}

#[test]
fn closure_retries_a_name_through_a_later_members_own_link() {
    // acme-app declares acme-shared but does NOT link it; acme-mid links it itself.
    // The ensure pass must still index acme-shared — a failed locate from one source
    // must not block the name for a later source with its own link.
    let ws = diamond(true, true);
    // Break acme-app's own acme-shared link so only acme-mid's remains.
    std::fs::remove_file(ws.root("acme-app").join(".vaire/packages/acme-shared")).unwrap();
    let _ = std::fs::remove_file(ws.root("shared-v1").join(".vaire/index.db"));

    let out = commands::index::run(&ws.ctx("acme-app"), false, false, false, false).unwrap();
    assert!(
        out.dependencies
            .iter()
            .any(|d| d.name == "acme-shared" && d.status == "indexed"),
        "located via acme-mid's own link: {:?}",
        out.dependencies
    );
    assert!(ws.root("shared-v1").join(".vaire/index.db").exists());
}

#[test]
fn corrupt_dep_index_is_rebuilt_not_fatal() {
    // The index is a disposable cache: garbage in a dep's index.db must trigger a clean
    // rebuild during the ensure pass, never abort the run.
    let ws = Ws::acceptance();
    std::fs::write(
        ws.root("acme-core").join(".vaire/index.db"),
        b"not a database",
    )
    .unwrap();

    let out = commands::index::run(&ws.ctx("acme-web"), false, false, false, false).unwrap();
    assert!(
        out.dependencies
            .iter()
            .any(|d| d.name == "acme-core" && d.status == "indexed"),
        "{:?}",
        out.dependencies
    );
    // And the rebuilt index actually answers.
    let resolved = commands::resolve::run(&ws.ctx("acme-web"), "@acme-core/team:platform").unwrap();
    assert_eq!(resolved.frontmatter["name"], "Platform Team");
}

#[test]
fn no_deps_skips_the_ensure_pass() {
    let ws = Ws::acceptance();
    for pkg in ["acme-core", "acme-shared"] {
        let _ = std::fs::remove_file(ws.root(pkg).join(".vaire/index.db"));
    }
    let out = commands::index::run(&ws.ctx("acme-web"), false, false, false, true).unwrap();
    assert!(out.dependencies.is_empty());
    assert!(!ws.root("acme-core").join(".vaire/index.db").exists());
    assert!(!ws.root("acme-shared").join(".vaire/index.db").exists());
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
