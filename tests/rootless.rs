//! Reading without a package to stand in (cli.md §6.8).
//!
//! The author's model — scope is my manifest's dependency closure — serves the person
//! writing a package. It has nothing to offer the larger audience who authors nothing and
//! wants to ask questions across everything they have. That is a second kind of session,
//! scoped by the catalog, and these tests pin both what it can do and the line it must not
//! cross: an author's resolution never falls back to it.

mod common;

use std::path::Path;

use common::Ws;
use vaire::catalog::{Catalog, Origin};
use vaire::commands::Ctx;
use vaire::config::Config;

/// Record packages in `ws`'s catalog, as ambient registration or a scan would have.
fn catalog(ws: &Ws, dirs: &[&str]) {
    let catalog = Catalog::open(&ws.home()).expect("catalog");
    for dir in dirs {
        let root = ws.root(dir);
        let config = Config::load(&root.join("knowledge.toml")).expect("manifest");
        catalog
            .record(&root, &config.name, &config.version, Origin::Scanned)
            .expect("record");
    }
}

/// A rootless context over `ws`'s catalog.
fn rootless(ws: &Ws) -> Ctx {
    Ctx::rootless(ws.home()).expect("rootless context")
}

/// Two unrelated packages — neither depends on the other, which is the point: a reader
/// sees both, an author would see only what it declared.
fn two_strangers() -> Ws {
    let ws = Ws::new();
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[])
        .add_file(
            "know/acme-core",
            "knowledge/platform.md",
            "---\nid: platform\ntype: team\nname: Platform Team\n---\n# Platform Team\n\nRuns the shared ingest pipeline.\n",
        )
        .commit("know/acme-core")
        .build("know/acme-core");
    ws.add_package_named("know/acme-wiki", "acme-wiki", &["page"], &[])
        .add_file(
            "know/acme-wiki",
            "knowledge/onboarding.md",
            "---\nid: onboarding\ntype: page\nname: Onboarding\n---\n# Onboarding\n\nHow to join the ingest pipeline rota.\n",
        )
        .commit("know/acme-wiki")
        .build("know/acme-wiki");
    catalog(&ws, &["know/acme-core", "know/acme-wiki"]);
    ws
}

// ---- the fan-out ------------------------------------------------------------

#[test]
fn search_spans_every_catalogued_package_with_no_package_to_stand_in() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let out = vaire::commands::search::run(&ctx, "ingest pipeline", None, None, Some(10), false)
        .expect("search");

    let ids: Vec<&str> = out.results.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains(&"@acme-core/team:platform"),
        "hits from both strangers: {ids:?}"
    );
    assert!(ids.contains(&"@acme-wiki/page:onboarding"), "{ids:?}");
    // Every result is package-qualified: with no package you are standing in, nothing is
    // local, so a bare id would be an address nobody could use.
    assert!(ids.iter().all(|id| id.starts_with('@')), "{ids:?}");
}

#[test]
fn suggest_spans_the_catalog_too() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let out = vaire::commands::suggest::run(&ctx, "platform team", None, Some(5), false)
        .expect("suggest");

    let ids: Vec<&str> = out.suggestions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"@acme-core/team:platform"), "{ids:?}");
}

#[test]
fn a_package_with_no_index_is_surfaced_not_fatal() {
    let ws = two_strangers();
    // A third package the catalog knows but nobody has indexed.
    ws.add_package_named("know/acme-draft", "acme-draft", &["page"], &[]);
    catalog(&ws, &["know/acme-draft"]);
    let ctx = rootless(&ws);

    let out = vaire::commands::search::run(&ctx, "ingest", None, None, Some(10), false)
        .expect("one unreadable package does not fail the query");

    assert!(
        out.skipped.contains(&"acme-draft".to_string()),
        "reported, never silently dropped: {:?}",
        out.skipped
    );
    assert!(!out.results.is_empty(), "the readable ones still answer");
}

// ---- point reads ------------------------------------------------------------

#[test]
fn a_qualified_id_resolves_against_the_catalog() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let out = vaire::commands::resolve::run(&ctx, "@acme-core/team:platform").expect("resolve");

    assert_eq!(out.id, "@acme-core/team:platform");
    assert_eq!(out.package.as_deref(), Some("acme-core"));
}

#[test]
fn a_bare_id_says_it_has_no_package_to_be_relative_to() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let err = vaire::commands::resolve::run(&ctx, "team:platform").expect_err("bare id");

    let msg = err.to_string();
    // Not "no node with id" — the node exists; the *address* is unaskable here, and
    // saying it is missing would send someone looking for the wrong problem.
    assert!(msg.contains("bare id"), "{msg}");
    assert!(msg.contains("@<package>/team:platform"), "the fix: {msg}");
}

#[test]
fn an_unknown_package_points_at_the_catalog_not_at_a_manifest() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let err = vaire::commands::resolve::run(&ctx, "@nope/team:x").expect_err("unknown package");

    let msg = err.to_string();
    assert!(msg.contains("catalog"), "{msg}");
    assert!(
        !msg.contains("knowledge.toml") && !msg.contains("[dependencies]"),
        "a reader has no manifest to be told to fix: {msg}"
    );
}

#[test]
fn backlinks_reach_across_packages_that_never_declared_each_other() {
    let ws = Ws::new();
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[])
        .add_file(
            "know/acme-core",
            "knowledge/platform.md",
            "---\nid: platform\ntype: team\n---\n# Platform\n",
        )
        .commit("know/acme-core")
        .build("know/acme-core");
    // acme-wiki *does* declare acme-core (a cross-package reference has to be declared by
    // its author to exist at all) — but nothing declares acme-wiki, and the reader finds
    // it anyway because the catalog, not a manifest, is the scope.
    ws.add_package_named(
        "know/acme-wiki",
        "acme-wiki",
        &["page"],
        &[("acme-core", "^1")],
    )
    .add_file(
        "know/acme-wiki",
        "knowledge/onboarding.md",
        "---\nid: onboarding\ntype: page\n---\n# Onboarding\n\nOwned by [[@acme-core/team:platform]].\n",
    )
    .link_to("know/acme-wiki", "acme-core", "know/acme-core")
    .commit("know/acme-wiki")
    .build("know/acme-wiki");
    catalog(&ws, &["know/acme-core", "know/acme-wiki"]);
    let ctx = rootless(&ws);

    let out = vaire::commands::backlinks::run(&ctx, "@acme-core/team:platform", None, None)
        .expect("backlinks");

    let ids: Vec<&str> = out.backlinks.iter().map(|b| b.id.as_str()).collect();
    assert!(ids.contains(&"@acme-wiki/page:onboarding"), "{ids:?}");
}

// ---- the line author mode must not cross ------------------------------------

#[test]
fn an_authors_undeclared_reference_still_fails_with_the_catalog_full() {
    let ws = two_strangers();
    // acme-app references acme-core without declaring it. The catalog knows exactly where
    // acme-core is — and that must change nothing: an undeclared reference is a manifest
    // error, and resolving it from ambient machine state is how a manifest stops meaning
    // anything to the next person who clones the package.
    ws.add_package_named("app", "acme-app", &["service"], &[])
        .add_file(
            "app",
            "knowledge/checkout.md",
            "---\nid: checkout\ntype: service\n---\n# Checkout\n\nOwned by [[@acme-core/team:platform]].\n",
        )
        .commit("app")
        .build("app");

    let ctx = ws.ctx("app");
    let err = vaire::commands::resolve::run(&ctx, "@acme-core/team:platform")
        .expect_err("an undeclared package must not resolve, catalog or no catalog");

    let msg = err.to_string();
    assert!(msg.contains("not a declared dependency"), "{msg}");
}

#[test]
fn an_authors_declared_but_unlinked_dependency_is_not_rescued_by_the_catalog() {
    let ws = two_strangers();
    // Declared this time, but never linked, and the ensure pass has not run. The catalog
    // could answer it — resolution must not, because a read command that materialized
    // scope out of machine state would make `vaire check` a different question on every
    // machine.
    ws.add_package_named("app", "acme-app", &["service"], &[("acme-core", "^1")])
        .add_file(
            "app",
            "knowledge/checkout.md",
            "---\nid: checkout\ntype: service\n---\n# Checkout\n\nOwned by [[@acme-core/team:platform]].\n",
        )
        .commit("app")
        .build("app");

    let ctx = ws.ctx("app");
    let err =
        vaire::commands::resolve::run(&ctx, "@acme-core/team:platform").expect_err("not linked");

    let msg = err.to_string();
    assert!(msg.contains("not linked"), "{msg}");
}

#[test]
fn local_is_refused_where_there_is_no_local() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let err = vaire::commands::search::run(&ctx, "ingest", None, None, Some(10), true)
        .expect_err("--local needs a package");

    assert!(err.to_string().contains("--local"), "{err}");
}

// ---- through the real binary ------------------------------------------------

fn vaire_cli(ws: &Ws, config_home: &Path, cwd: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_vaire"));
    cmd.env("VAIRE_CONFIG_HOME", config_home)
        .env("VAIRE_HOME", ws.home())
        .env_remove("VAIRE_REPO")
        .current_dir(cwd)
        .arg("--no-color");
    cmd
}

#[test]
fn the_cli_falls_back_to_the_catalog_outside_any_package() {
    let ws = two_strangers();
    let home = tempfile::tempdir().unwrap();
    // A directory with no package above it anywhere.
    let nowhere = tempfile::tempdir().unwrap();

    let out = vaire_cli(&ws, home.path(), nowhere.path())
        .args(["--json", "search", "ingest pipeline"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.iter().any(|id| id.starts_with("@acme-core/")),
        "{ids:?}"
    );
    assert!(
        ids.iter().any(|id| id.starts_with("@acme-wiki/")),
        "{ids:?}"
    );
}

#[test]
fn all_reaches_past_the_closure_from_inside_a_package() {
    let ws = two_strangers();
    let home = tempfile::tempdir().unwrap();
    // acme-app declares nothing, so its closure is itself.
    ws.add_package_named("app", "acme-app", &["service"], &[])
        .add_file(
            "app",
            "knowledge/checkout.md",
            "---\nid: checkout\ntype: service\n---\n# Checkout\n",
        )
        .commit("app")
        .build("app");

    let scoped = vaire_cli(&ws, home.path(), &ws.root("app"))
        .args(["--json", "search", "ingest pipeline"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    assert_eq!(
        json["results"].as_array().unwrap().len(),
        0,
        "the closure is just this package, and it says nothing about ingest"
    );

    let all = vaire_cli(&ws, home.path(), &ws.root("app"))
        .args(["--json", "search", "ingest pipeline", "--all"])
        .output()
        .unwrap();
    assert!(
        all.status.success(),
        "{}",
        String::from_utf8_lossy(&all.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&all.stdout).unwrap();
    let ids: Vec<&str> = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"@acme-core/team:platform"), "{ids:?}");
}

#[test]
fn maintain_commands_still_need_a_package() {
    let ws = two_strangers();
    let home = tempfile::tempdir().unwrap();
    let nowhere = tempfile::tempdir().unwrap();

    let out = vaire_cli(&ws, home.path(), nowhere.path())
        .arg("index")
        .output()
        .unwrap();

    assert!(!out.status.success(), "there is nothing here to maintain");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no corpus found"), "{stderr}");
}

#[test]
fn mcp_serves_the_catalog_when_started_outside_a_package() {
    let ws = two_strangers();
    let home = tempfile::tempdir().unwrap();
    let nowhere = tempfile::tempdir().unwrap();

    let mut child = vaire_cli(&ws, home.path(), nowhere.path())
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"search","arguments":{{"query":"ingest pipeline"}}}}}}"#
        )
        .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The agent surface is the whole point: pointed at the machine, not at a checkout.
    assert!(stdout.contains("@acme-core/team:platform"), "{stdout}");
}

// ---- the scope's edges ------------------------------------------------------

#[test]
fn all_keeps_the_package_you_are_standing_in_even_when_uncatalogued() {
    let ws = two_strangers();
    let home = tempfile::tempdir().unwrap();
    // acme-app is deliberately never catalogued. Reads record no sightings, so a checkout
    // nobody has indexed, checked, or `catalog add`ed is genuinely absent from it — and
    // `--all` widens the query rather than redirecting it, so "search everything" that
    // silently excluded *here* would be the one answer nobody would read as correct.
    ws.add_package_named("app", "acme-app", &["service"], &[])
        .add_file(
            "app",
            "knowledge/checkout.md",
            "---\nid: checkout\ntype: service\n---\n# Checkout\n\nFeeds the shared ingest pipeline.\n",
        )
        .commit("app")
        .build("app");

    let out = vaire_cli(&ws, home.path(), &ws.root("app"))
        .args(["--json", "search", "ingest pipeline", "--all"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"@acme-app/service:checkout"),
        "widened, not redirected: {ids:?}"
    );
    assert!(ids.contains(&"@acme-core/team:platform"), "{ids:?}");
    // Still qualified: `--all` is the rootless scope, where nothing is local.
    assert!(ids.iter().all(|id| id.starts_with('@')), "{ids:?}");
}

#[test]
fn fan_out_stops_at_the_catalog_and_does_not_follow_links_out_of_it() {
    let ws = Ws::new();
    ws.add_package_named("know/acme-secret", "acme-secret", &["team"], &[])
        .add_file(
            "know/acme-secret",
            "knowledge/skunkworks.md",
            "---\nid: skunkworks\ntype: team\n---\n# Skunkworks\n\nRuns the shared ingest pipeline.\n",
        )
        .commit("know/acme-secret")
        .build("know/acme-secret");
    // acme-wiki declares and links acme-secret, so its own references resolve. But nobody
    // catalogued acme-secret, and a reader's scope is the catalog: whether a package shows
    // up in a search must not depend on wiring inside some *other* package.
    ws.add_package_named(
        "know/acme-wiki",
        "acme-wiki",
        &["page"],
        &[("acme-secret", "^1")],
    )
    .add_file(
        "know/acme-wiki",
        "knowledge/onboarding.md",
        "---\nid: onboarding\ntype: page\n---\n# Onboarding\n\nJoin the shared ingest pipeline rota.\n",
    )
    .link_to("know/acme-wiki", "acme-secret", "know/acme-secret")
    .commit("know/acme-wiki")
    .build("know/acme-wiki");
    catalog(&ws, &["know/acme-wiki"]);
    let ctx = rootless(&ws);

    let out = vaire::commands::search::run(&ctx, "ingest pipeline", None, None, Some(10), false)
        .expect("search");

    let ids: Vec<&str> = out.results.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"@acme-wiki/page:onboarding"), "{ids:?}");
    assert!(
        !ids.iter().any(|id| id.starts_with("@acme-secret/")),
        "reachable through acme-wiki's links, but not in the catalog: {ids:?}"
    );
    assert!(
        !out.skipped.contains(&"acme-secret".to_string()),
        "not skipped either — it was never in scope to skip: {:?}",
        out.skipped
    );
}

#[test]
fn two_checkouts_declaring_one_name_are_refused_not_ranked() {
    let ws = two_strangers();
    // A fork beside its original. Picking the higher version would be a guess dressed as
    // arithmetic, and a fork is routinely newer than what it forked from.
    ws.add_package_named("fork/acme-core", "acme-core", &["team"], &[])
        .add_file(
            "fork/acme-core",
            "knowledge/platform.md",
            "---\nid: platform\ntype: team\n---\n# Platform\n",
        )
        .commit("fork/acme-core")
        .build("fork/acme-core");
    catalog(&ws, &["fork/acme-core"]);
    let ctx = rootless(&ws);

    let err = vaire::commands::resolve::run(&ctx, "@acme-core/team:platform")
        .expect_err("two live checkouts declare acme-core");

    let msg = err.to_string();
    assert!(msg.contains("more than one"), "{msg}");
    assert!(msg.contains("catalog rm"), "names a way out: {msg}");
    assert!(
        msg.contains("fork") && msg.contains("know"),
        "names both paths, or it cannot be acted on: {msg}"
    );
}

#[test]
fn the_catalog_is_not_shadowed_by_whatever_sits_beside_the_shell() {
    let ws = two_strangers();
    let home = tempfile::tempdir().unwrap();
    // A directory that is not a package but happens to hold a `.vaire/packages/` entry —
    // a stale derived directory, or another tool's. The synthetic rootless root has no
    // directory of its own, so a relative probe would resolve against the process working
    // directory and let this answer in the catalog's place. A reader's scope must not
    // depend on which directory they ran from.
    let nowhere = tempfile::tempdir().unwrap();
    let packages = nowhere.path().join(".vaire").join("packages");
    std::fs::create_dir_all(&packages).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(ws.root("know/acme-wiki"), packages.join("acme-core")).unwrap();

    let out = vaire_cli(&ws, home.path(), nowhere.path())
        .args(["--json", "resolve", "@acme-core/team:platform"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "the catalog answers, not the decoy beside the shell: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["package"].as_str(), Some("acme-core"));
}

// ---- the catalog's own state ------------------------------------------------

#[test]
fn a_path_that_answers_again_is_promoted_back_to_live() {
    let ws = two_strangers();
    let root = ws.root("know/acme-core");
    {
        // Marked missing by an earlier run (the checkout was away), and now it is back.
        let catalog = Catalog::open(&ws.home()).expect("catalog");
        catalog
            .set_state(&root, vaire::catalog::State::Missing)
            .expect("demote");
    }

    {
        let ctx = rootless(&ws);
        vaire::commands::resolve::run(&ctx, "@acme-core/team:platform")
            .expect("a returned checkout still answers");
    }

    let catalog = Catalog::open(&ws.home()).expect("catalog");
    let sighting = catalog
        .sightings()
        .expect("sightings")
        .into_iter()
        .find(|s| s.name == "acme-core")
        .expect("row");
    // Demoting without promoting would leave `catalog rm --missing` deleting the row for a
    // package the session had just served.
    assert_eq!(
        sighting.state,
        vaire::catalog::State::Live,
        "two states with no clocks only works if both transitions are taken"
    );
}

// ---- commands that have no rootless form ------------------------------------

#[test]
fn unresolved_widens_to_the_catalog_instead_of_failing() {
    let ws = two_strangers();
    ws.add_file(
        "know/acme-wiki",
        "knowledge/rota.md",
        "---\nid: rota\ntype: page\n---\n# Rota\n\nOwned by [[?team: Platform]].\n",
    )
    .commit("know/acme-wiki")
    .build("know/acme-wiki");
    let ctx = rootless(&ws);

    // Its default scope is the current package, and there is none — so the widened form
    // is the only one there is, rather than an error about a missing index.
    let out = vaire::commands::unresolved::run(&ctx, None, None, false).expect("unresolved");

    let records: Vec<&str> = out.unresolved.iter().map(|i| i.record.as_str()).collect();
    assert!(
        records.iter().any(|r| r.starts_with("@acme-wiki/")),
        "package-qualified, like every rootless result: {records:?}"
    );
}

#[test]
fn unresolved_refuses_a_scope_with_no_package_to_be_relative_to() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let err = vaire::commands::unresolved::run(&ctx, None, Some("team:platform"), false)
        .expect_err("a scope is one package's container");

    assert!(err.to_string().contains("--scope"), "{err}");
}

#[test]
fn deps_has_no_rootless_form_and_says_so() {
    let ws = two_strangers();
    let ctx = rootless(&ws);

    let err = vaire::commands::deps::run(&ctx).expect_err("no package to report on");

    let msg = err.to_string();
    // Not an empty tree named "": it reports *a package's* links, and there is none here.
    assert!(msg.contains("catalog list"), "points somewhere real: {msg}");
}

#[test]
fn an_explicit_repo_that_is_not_a_package_is_reported_not_widened() {
    let ws = two_strangers();
    let home = tempfile::tempdir().unwrap();
    let nowhere = tempfile::tempdir().unwrap();

    let out = vaire_cli(&ws, home.path(), nowhere.path())
        .args([
            "--json",
            "--repo",
            nowhere.path().to_str().unwrap(),
            "search",
            "ingest pipeline",
        ])
        .output()
        .unwrap();

    // Both arrive as `NoRepo`, and only the ambient one may be answered with a different
    // scope — otherwise a typo'd override silently returns results from the whole machine.
    assert!(
        !out.status.success(),
        "a bad --repo must not fall through to the catalog: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
