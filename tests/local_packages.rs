//! Satisfying declared dependencies from the local-packages root (cli.md §6.3).
//!
//! Two layers. The discovery engine is driven in-process with an explicit root, which
//! keeps these tests hermetic and parallel-safe (the global user config is shared by every
//! in-process test). The wiring — that `index`/`add` consult the *configured* root, and
//! that read commands never link — is driven through the real binary with its own
//! `VAIRE_CONFIG_HOME`.

mod common;

use std::path::{Path, PathBuf};

use common::Ws;
use vaire::config::Config;
use vaire::corpus::repo::Repo;
use vaire::workspace::discover::{self, Satisfied};

/// Run a discovery pass for one member against `root`.
fn satisfy(ws: &Ws, pkg: &str, root: Option<&Path>) -> Satisfied {
    let member = ws.root(pkg);
    let repo = Repo::discover(Some(&member), &member).expect("repo");
    let config = Config::load(&member.join("knowledge.toml")).expect("manifest");
    discover::satisfy(&repo, &config, root)
}

/// The `.vaire/packages/<dep>` entry of a member.
fn entry(ws: &Ws, pkg: &str, dep: &str) -> PathBuf {
    ws.root(pkg).join(".vaire/packages").join(dep)
}

// ---- matching by declared name ---------------------------------------------

#[test]
fn a_package_nested_inside_a_bigger_repo_is_found_by_its_declared_name() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-handbook", "^1")]);
    // The knowledge base is one component of a larger repo, and no directory on the way
    // down is named after the package — only its manifest says what it is.
    ws.add_package_named(
        "know/platform-docs/docs/kb",
        "acme-handbook",
        &["guide"],
        &[],
    );

    let root = ws.dir.path().join("know");
    let out = satisfy(&ws, "acme-web", Some(&root));

    assert_eq!(out.linked.len(), 1, "one link created: {out:?}");
    assert_eq!(out.linked[0].name, "acme-handbook");
    assert!(out.notes.is_empty(), "nothing unresolved: {out:?}");
    assert_eq!(
        std::fs::canonicalize(entry(&ws, "acme-web", "acme-handbook")).unwrap(),
        std::fs::canonicalize(ws.root("know/platform-docs/docs/kb")).unwrap(),
        "the link points at the nested package"
    );
}

#[test]
fn a_transitive_dependency_is_linked_at_the_run_root_not_inside_its_consumer() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named(
        "know/acme-core",
        "acme-core",
        &["team"],
        &[("acme-shared", "^1")],
    );
    ws.add_package_named("know/acme-shared", "acme-shared", &["site"], &[]);

    let root = ws.dir.path().join("know");
    let out = satisfy(&ws, "acme-web", Some(&root));

    // acme-shared is only visible once acme-core is linked — the pass repeats until it
    // stops making progress.
    let mut linked: Vec<&str> = out.linked.iter().map(|l| l.name.as_str()).collect();
    linked.sort();
    assert_eq!(linked, ["acme-core", "acme-shared"], "{out:?}");

    // Both live in the run-root's own links; the dependency's directory is never written to.
    assert!(entry(&ws, "acme-web", "acme-shared").exists());
    assert!(
        !ws.root("know/acme-core")
            .join(".vaire/packages/acme-shared")
            .exists(),
        "discovery never writes into a dependency's directory"
    );
}

// ---- refusing to guess ------------------------------------------------------

#[test]
fn two_packages_declaring_one_name_are_never_guessed_between() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.add_package_named("know/acme-core-fork", "acme-core", &["team"], &[]);

    let root = ws.dir.path().join("know");
    let out = satisfy(&ws, "acme-web", Some(&root));

    assert!(out.linked.is_empty(), "nothing linked: {out:?}");
    let note = out.notes.get("acme-core").expect("a note explains why");
    assert!(note.contains("more than one"), "{note}");
    // Both candidates are named, so the user can pick one.
    assert!(note.contains("acme-core-fork"), "{note}");
    assert!(!entry(&ws, "acme-web", "acme-core").exists());
}

#[test]
fn a_name_that_is_nowhere_under_the_root_reports_where_it_looked() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    std::fs::create_dir_all(ws.dir.path().join("know")).unwrap();

    let root = ws.dir.path().join("know");
    let out = satisfy(&ws, "acme-web", Some(&root));

    let note = out.notes.get("acme-core").expect("a note explains why");
    assert!(note.contains("no package declaring 'acme-core'"), "{note}");
    assert!(note.contains("know"), "the root that was searched: {note}");
}

// ---- only gaps are filled ---------------------------------------------------

#[test]
fn an_explicit_link_is_never_rewritten_by_discovery() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.add_package_named("fork/acme-core", "acme-core", &["team"], &[]);
    // The user deliberately linked the fork, which lives outside the local-packages root.
    ws.link_to("acme-web", "acme-core", "fork/acme-core");

    let root = ws.dir.path().join("know");
    let out = satisfy(&ws, "acme-web", Some(&root));

    assert!(
        out.linked.is_empty(),
        "an existing link is left alone: {out:?}"
    );
    assert_eq!(
        std::fs::canonicalize(entry(&ws, "acme-web", "acme-core")).unwrap(),
        std::fs::canonicalize(ws.root("fork/acme-core")).unwrap(),
        "still the fork the user chose"
    );
}

#[test]
fn a_broken_link_is_healed_by_re_discovery() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.link_to("acme-web", "acme-core", "know/acme-core");

    // The package moves: the link now points at nothing.
    std::fs::rename(ws.root("know/acme-core"), ws.root("know/core-moved")).unwrap();
    assert!(std::fs::canonicalize(entry(&ws, "acme-web", "acme-core")).is_err());

    let root = ws.dir.path().join("know");
    let out = satisfy(&ws, "acme-web", Some(&root));

    assert_eq!(out.linked.len(), 1, "the broken entry is replaced: {out:?}");
    assert_eq!(
        std::fs::canonicalize(entry(&ws, "acme-web", "acme-core")).unwrap(),
        std::fs::canonicalize(ws.root("know/core-moved")).unwrap(),
        "healed to where the package went"
    );
}

#[test]
fn without_a_configured_root_nothing_is_discovered() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);

    let out = satisfy(&ws, "acme-web", None);

    assert!(out.linked.is_empty());
    assert!(out.notes.is_empty());
    assert!(
        !ws.root("acme-web").join(".vaire/packages").exists(),
        "not even the packages dir is created"
    );
}

#[test]
fn a_fully_linked_package_never_consults_the_root() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.link_to("acme-web", "acme-core", "know/acme-core");

    // The root does not even exist. Nothing is missing, so it is never looked at — the
    // steady state must not pay for a filesystem walk on every `vaire index`.
    let out = satisfy(&ws, "acme-web", Some(&ws.dir.path().join("nowhere")));

    assert!(out.linked.is_empty());
    assert!(out.notes.is_empty());
    assert!(
        out.warnings.is_empty(),
        "an unusable root is not even validated when nothing needs it: {out:?}"
    );
}

#[test]
fn satisfy_name_links_just_the_one_dependency() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.add_package_named("know/acme-shared", "acme-shared", &["site"], &[]);

    let root = ws.dir.path().join("know");
    let out = discover::satisfy_name(&ws.root("acme-web"), "acme-core", Some(&root));

    assert_eq!(out.linked.len(), 1);
    assert_eq!(out.linked[0].name, "acme-core");
    assert!(
        !entry(&ws, "acme-web", "acme-shared").exists(),
        "`vaire add` wires the dependency it declared, not the whole root"
    );
}

// ---- the wiring, through the real binary ------------------------------------

/// A workspace whose consumer and local-packages root are ready for the CLI: every member
/// has a node and a commit, so `vaire index` has something to build.
fn cli_workspace() -> Ws {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")])
        .add_file(
            "acme-web",
            "knowledge/checkout.md",
            "---\nid: checkout\ntype: service\n---\n# Checkout\n\nOwned by [[@acme-core/team:platform]].\n",
        )
        .commit("acme-web");
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[])
        .add_file(
            "know/acme-core",
            "knowledge/platform.md",
            "---\nid: platform\ntype: team\n---\n# Platform\n",
        )
        .commit("know/acme-core");
    ws
}

fn vaire_cli(ws: &Ws, home: &Path, pkg: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_vaire"));
    cmd.env("VAIRE_CONFIG_HOME", home)
        .current_dir(ws.root(pkg))
        .arg("--no-color");
    cmd
}

#[test]
fn index_satisfies_declared_dependencies_from_the_configured_root() {
    let ws = cli_workspace();
    let home = tempfile::tempdir().unwrap();
    let root = ws.dir.path().join("know");

    let ok = vaire_cli(&ws, home.path(), "acme-web")
        .args(["configure", "local-packages"])
        .arg(&root)
        .status()
        .unwrap();
    assert!(ok.success());

    // A fresh clone's whole wiring step: `vaire index`.
    let out = vaire_cli(&ws, home.path(), "acme-web")
        .args(["--json", "index"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let dep = &json["dependencies"][0];
    assert_eq!(dep["name"], "acme-core");
    assert_eq!(dep["status"], "indexed");
    assert!(
        dep["linked"].as_str().unwrap().ends_with("acme-core"),
        "the run reports the link it created: {dep}"
    );

    // The cross-package reference now resolves — the point of all of this.
    let resolved = vaire_cli(&ws, home.path(), "acme-web")
        .args(["--json", "resolve", "@acme-core/team:platform"])
        .output()
        .unwrap();
    assert!(resolved.status.success());
    let json: serde_json::Value = serde_json::from_slice(&resolved.stdout).unwrap();
    assert_eq!(json["package"], "acme-core");
}

#[test]
fn add_reports_the_link_discovery_made_and_a_read_makes_none() {
    let ws = cli_workspace();
    let home = tempfile::tempdir().unwrap();
    let root = ws.dir.path().join("know");
    vaire_cli(&ws, home.path(), "acme-web")
        .args(["configure", "local-packages"])
        .arg(&root)
        .status()
        .unwrap();

    // `vaire add` declares — and reports the link it could satisfy from the root.
    let out = vaire_cli(&ws, home.path(), "acme-web")
        .args(["--json", "add", "acme-core"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["discovered"], true);
    assert!(json["linked"].as_str().unwrap().ends_with("acme-core"));

    // Reads never write links: build, drop the entry, then query with discovery still
    // configured — the query must tolerate the missing dependency, not re-link it.
    let built = vaire_cli(&ws, home.path(), "acme-web")
        .arg("index")
        .status()
        .unwrap();
    assert!(built.success());
    std::fs::remove_file(entry(&ws, "acme-web", "acme-core")).unwrap();
    let read = vaire_cli(&ws, home.path(), "acme-web")
        .args(["--json", "search", "checkout"])
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(
        !entry(&ws, "acme-web", "acme-core").exists(),
        "a read command must never materialize a link"
    );
}
