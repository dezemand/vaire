//! Satisfying declared dependencies from the catalog (registry.v2.md §6).
//!
//! Two layers. The selection engine is driven in-process against a hermetic catalog, which
//! keeps these tests parallel-safe (Turso locks a database exclusively on open, so a shared
//! catalog would serialize them). The wiring — that `index`/`add` consult the catalog, that
//! read commands never link, and that the retired `local-packages` root migrates itself —
//! is driven through the real binary with its own `VAIRE_HOME`.

mod common;

use std::path::{Path, PathBuf};

use common::Ws;
use vaire::catalog::{Catalog, Origin};
use vaire::config::Config;
use vaire::corpus::repo::Repo;
use vaire::workspace::satisfy::{self, Satisfied};

/// Record packages in `ws`'s catalog as a scan would have found them.
fn catalog_scanned(ws: &Ws, dirs: &[&str]) {
    let catalog = Catalog::open(&ws.home()).expect("catalog");
    for dir in dirs {
        let root = ws.root(dir);
        let config = Config::load(&root.join("knowledge.toml")).expect("manifest");
        catalog
            .record(&root, &config.name, &config.version, Origin::Scanned)
            .expect("record");
    }
}

/// Record one package as an explicit `vaire catalog add` would.
fn catalog_registered(ws: &Ws, dir: &str) {
    let catalog = Catalog::open(&ws.home()).expect("catalog");
    let root = ws.root(dir);
    let config = Config::load(&root.join("knowledge.toml")).expect("manifest");
    catalog
        .record(&root, &config.name, &config.version, Origin::Registered)
        .expect("record");
}

/// Rewrite a package's declared version, leaving everything else alone.
fn set_version(ws: &Ws, dir: &str, version: &str) {
    let manifest = ws.root(dir).join("knowledge.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let rewritten: String = text
        .lines()
        .map(|line| match line.starts_with("version") {
            true => format!("version = \"{version}\""),
            false => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&manifest, rewritten + "\n").unwrap();
}

/// Rewrite a package's declared name — identity is declared, so this really is a different
/// package afterwards.
fn set_name(ws: &Ws, dir: &str, name: &str) {
    let manifest = ws.root(dir).join("knowledge.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let rewritten: String = text
        .lines()
        .map(|line| match line.starts_with("name") {
            true => format!("name = \"{name}\""),
            false => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&manifest, rewritten + "\n").unwrap();
}

/// Run a satisfy pass for one member against this workspace's catalog.
fn satisfy(ws: &Ws, pkg: &str) -> Satisfied {
    let member = ws.root(pkg);
    let repo = Repo::discover(Some(&member), &member).expect("repo");
    let config = Config::load(&member.join("knowledge.toml")).expect("manifest");
    satisfy::satisfy(&repo, &config, &ws.home())
}

/// The `.vaire/packages/<dep>` entry of a member.
fn entry(ws: &Ws, pkg: &str, dep: &str) -> PathBuf {
    ws.root(pkg).join(".vaire/packages").join(dep)
}

fn linked_target(ws: &Ws, pkg: &str, dep: &str) -> PathBuf {
    std::fs::canonicalize(entry(ws, pkg, dep)).expect("a resolvable link")
}

// ---- the constraint is now a selector ---------------------------------------

#[test]
fn two_majors_of_one_package_are_selected_between_not_reported_as_ambiguous() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/core-v1", "acme-core", &["team"], &[]);
    ws.add_package_named("know/core-v2", "acme-core", &["team"], &[]);
    set_version(&ws, "know/core-v2", "2.0.0");
    catalog_scanned(&ws, &["know/core-v1", "know/core-v2"]);

    let out = satisfy(&ws, "acme-web");

    // v0.2.0 called this an ambiguity, because it matched on name alone. It never was one:
    // the consumer said which major line it wanted.
    assert_eq!(out.linked.len(), 1, "one link created: {out:?}");
    assert!(out.notes.is_empty(), "nothing unresolved: {out:?}");
    assert_eq!(
        linked_target(&ws, "acme-web", "acme-core"),
        std::fs::canonicalize(ws.root("know/core-v1")).unwrap(),
    );
}

#[test]
fn the_other_consumer_of_the_same_two_gets_the_other_major() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^2")]);
    ws.add_package_named("know/core-v1", "acme-core", &["team"], &[]);
    ws.add_package_named("know/core-v2", "acme-core", &["team"], &[]);
    set_version(&ws, "know/core-v2", "2.0.0");
    catalog_scanned(&ws, &["know/core-v1", "know/core-v2"]);

    satisfy(&ws, "acme-web");

    assert_eq!(
        linked_target(&ws, "acme-web", "acme-core"),
        std::fs::canonicalize(ws.root("know/core-v2")).unwrap(),
        "the ^2 consumer gets the 2.x copy, from the same catalog"
    );
}

#[test]
fn a_version_outside_the_constraint_reports_what_the_machine_actually_has() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^3")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    catalog_scanned(&ws, &["know/acme-core"]);

    let out = satisfy(&ws, "acme-web");

    let note = out.notes.get("acme-core").expect("a note explains why");
    assert!(note.contains("satisfies ^3"), "{note}");
    assert!(note.contains("1.0.0"), "the version it does have: {note}");
    assert!(!entry(&ws, "acme-web", "acme-core").exists());
}

#[test]
fn caret_zero_is_a_major_line_like_any_other() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^0")]);
    ws.add_package_named("know/core-old", "acme-core", &["team"], &[]);
    ws.add_package_named("know/core-new", "acme-core", &["team"], &[]);
    set_version(&ws, "know/core-old", "0.9.0");
    catalog_scanned(&ws, &["know/core-old", "know/core-new"]);

    satisfy(&ws, "acme-web");

    assert_eq!(
        linked_target(&ws, "acme-web", "acme-core"),
        std::fs::canonicalize(ws.root("know/core-old")).unwrap(),
        "^0 selects the 0.x copy, not the 1.x one"
    );
}

// ---- refusing to guess ------------------------------------------------------

#[test]
fn two_copies_in_one_major_line_are_never_guessed_between() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.add_package_named("know/acme-core-fork", "acme-core", &["team"], &[]);
    // The fork is *newer*, which is exactly why a version tiebreak would be wrong here:
    // a fork routinely outruns what it forked from.
    set_version(&ws, "know/acme-core-fork", "1.9.0");
    catalog_scanned(&ws, &["know/acme-core", "know/acme-core-fork"]);

    let out = satisfy(&ws, "acme-web");

    assert!(out.linked.is_empty(), "nothing linked: {out:?}");
    let note = out.notes.get("acme-core").expect("a note explains why");
    assert!(note.contains("more than one"), "{note}");
    assert!(note.contains("acme-core-fork"), "both are named: {note}");
    assert!(!entry(&ws, "acme-web", "acme-core").exists());
}

#[test]
fn an_explicit_registration_settles_an_otherwise_ambiguous_name() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.add_package_named("work/acme-core", "acme-core", &["team"], &[]);
    catalog_scanned(&ws, &["know/acme-core"]);
    // `vaire catalog add work/acme-core` — a statement of intent, and the way out of the
    // ambiguity above that does not mean editing every consumer's links.
    catalog_registered(&ws, "work/acme-core");

    let out = satisfy(&ws, "acme-web");

    assert!(out.notes.is_empty(), "no longer ambiguous: {out:?}");
    assert_eq!(
        linked_target(&ws, "acme-web", "acme-core"),
        std::fs::canonicalize(ws.root("work/acme-core")).unwrap(),
    );
}

#[test]
fn a_name_no_sighting_declares_says_how_to_feed_the_catalog() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);

    let out = satisfy(&ws, "acme-web");

    let note = out.notes.get("acme-core").expect("a note explains why");
    assert!(note.contains("no package declaring 'acme-core'"), "{note}");
    assert!(note.contains("vaire catalog add"), "the fix: {note}");
    assert!(note.contains("vaire catalog scan"), "the bulk fix: {note}");
}

// ---- the catalog is an index, never truth -----------------------------------

#[test]
fn a_version_changed_since_the_sighting_is_re_read_not_believed() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^2")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    catalog_scanned(&ws, &["know/acme-core"]); // recorded at 1.0.0
    // A release happened in the working copy after the sighting was taken.
    set_version(&ws, "know/acme-core", "2.1.0");

    let out = satisfy(&ws, "acme-web");

    assert_eq!(out.linked.len(), 1, "the fresh 2.1.0 satisfies ^2: {out:?}");
    // …and the row caught up, so the next lookup starts from what is true now.
    let catalog = Catalog::open(&ws.home()).unwrap();
    let row = catalog.by_name("acme-core").unwrap();
    assert_eq!(row[0].version, "2.1.0", "the sighting was corrected");
}

#[test]
fn a_package_renamed_on_disk_stops_being_a_candidate_and_the_row_follows_it() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    catalog_scanned(&ws, &["know/acme-core"]);
    set_name(&ws, "know/acme-core", "acme-platform");

    let out = satisfy(&ws, "acme-web");

    assert!(
        out.notes.contains_key("acme-core"),
        "it no longer declares that name: {out:?}"
    );
    let catalog = Catalog::open(&ws.home()).unwrap();
    assert!(catalog.by_name("acme-core").unwrap().is_empty());
    assert_eq!(
        catalog.by_name("acme-platform").unwrap().len(),
        1,
        "the row followed the package rather than being deleted"
    );
}

#[test]
fn a_path_that_no_longer_answers_is_marked_missing() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    catalog_scanned(&ws, &["know/acme-core"]);
    std::fs::remove_dir_all(ws.root("know/acme-core")).unwrap();

    let out = satisfy(&ws, "acme-web");

    assert!(out.notes.contains_key("acme-core"), "{out:?}");
    let catalog = Catalog::open(&ws.home()).unwrap();
    let row = &catalog.by_name("acme-core").unwrap()[0];
    assert_eq!(
        row.state,
        vaire::catalog::State::Missing,
        "the row records that it looked and found nothing"
    );
}

// ---- constraints across the closure -----------------------------------------

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
    catalog_scanned(&ws, &["know/acme-core", "know/acme-shared"]);

    let out = satisfy(&ws, "acme-web");

    // acme-shared is only visible once acme-core is linked — the pass repeats until it
    // stops making progress.
    let mut linked: Vec<&str> = out.linked.iter().map(|l| l.name.as_str()).collect();
    linked.sort();
    assert_eq!(linked, ["acme-core", "acme-shared"], "{out:?}");
    assert!(entry(&ws, "acme-web", "acme-shared").exists());
    assert!(
        !ws.root("know/acme-core")
            .join(".vaire/packages/acme-shared")
            .exists(),
        "the pass never writes into a dependency's directory"
    );
}

#[test]
fn two_members_wanting_disjoint_majors_is_a_reported_conflict() {
    let ws = Ws::new();
    // The run root wants ^1; its dependency wants ^2 of the same package. One directory is
    // linked per name, so there is no answer that satisfies both.
    ws.add_package(
        "acme-web",
        &["service"],
        &[("acme-core", "^1"), ("acme-shared", "^1")],
    );
    ws.add_package_named(
        "know/acme-shared",
        "acme-shared",
        &["site"],
        &[("acme-core", "^2")],
    );
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    catalog_scanned(&ws, &["know/acme-shared", "know/acme-core"]);

    let out = satisfy(&ws, "acme-web");

    let note = out
        .notes
        .get("acme-core")
        .expect("the conflict is reported");
    assert!(note.contains("incompatible"), "{note}");
    assert!(
        note.contains("acme-web wants ^1"),
        "both sides named: {note}"
    );
    assert!(note.contains("acme-shared wants ^2"), "{note}");
    assert!(
        !entry(&ws, "acme-web", "acme-core").exists(),
        "linking either one would betray the other declarer"
    );
}

#[test]
fn two_members_agreeing_on_a_major_resolve_once() {
    let ws = Ws::new();
    ws.add_package(
        "acme-web",
        &["service"],
        &[("acme-core", "^1"), ("acme-shared", "^1")],
    );
    ws.add_package_named(
        "know/acme-shared",
        "acme-shared",
        &["site"],
        &[("acme-core", "^1")],
    );
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    catalog_scanned(&ws, &["know/acme-shared", "know/acme-core"]);

    let out = satisfy(&ws, "acme-web");

    assert!(out.notes.is_empty(), "agreement is not a conflict: {out:?}");
    assert!(entry(&ws, "acme-web", "acme-core").exists());
}

// ---- only gaps are filled ---------------------------------------------------

#[test]
fn an_explicit_link_is_never_rewritten() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.add_package_named("fork/acme-core", "acme-core", &["team"], &[]);
    // The user deliberately linked the fork, which the catalog has never heard of.
    ws.link_to("acme-web", "acme-core", "fork/acme-core");
    catalog_scanned(&ws, &["know/acme-core"]);

    let out = satisfy(&ws, "acme-web");

    assert!(
        out.linked.is_empty(),
        "an existing link is left alone: {out:?}"
    );
    assert_eq!(
        linked_target(&ws, "acme-web", "acme-core"),
        std::fs::canonicalize(ws.root("fork/acme-core")).unwrap(),
        "still the fork the user chose"
    );
}

#[test]
fn a_broken_link_is_healed_from_the_catalog() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.link_to("acme-web", "acme-core", "know/acme-core");

    // The package moves: the link now points at nothing, and the catalog learns where it
    // went (as a `vaire index` in the moved copy would have recorded).
    std::fs::rename(ws.root("know/acme-core"), ws.root("know/core-moved")).unwrap();
    assert!(std::fs::canonicalize(entry(&ws, "acme-web", "acme-core")).is_err());
    catalog_scanned(&ws, &["know/core-moved"]);

    let out = satisfy(&ws, "acme-web");

    assert_eq!(out.linked.len(), 1, "the broken entry is replaced: {out:?}");
    assert_eq!(
        linked_target(&ws, "acme-web", "acme-core"),
        std::fs::canonicalize(ws.root("know/core-moved")).unwrap(),
        "healed to where the package went"
    );
}

#[test]
fn a_fully_linked_package_needs_nothing_from_the_catalog() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.link_to("acme-web", "acme-core", "know/acme-core");

    // The catalog is empty and never consulted: nothing is missing.
    let out = satisfy(&ws, "acme-web");

    assert!(out.linked.is_empty());
    assert!(out.notes.is_empty());
    assert!(out.warnings.is_empty(), "{out:?}");
}

#[test]
fn satisfy_name_links_just_the_one_dependency() {
    let ws = Ws::new();
    ws.add_package("acme-web", &["service"], &[("acme-core", "^1")]);
    ws.add_package_named("know/acme-core", "acme-core", &["team"], &[]);
    ws.add_package_named("know/acme-shared", "acme-shared", &["site"], &[]);
    catalog_scanned(&ws, &["know/acme-core", "know/acme-shared"]);

    let out = satisfy::satisfy_name(&ws.root("acme-web"), "acme-core", "^1", &ws.home());

    assert_eq!(out.linked.len(), 1);
    assert_eq!(out.linked[0].name, "acme-core");
    assert!(
        !entry(&ws, "acme-web", "acme-shared").exists(),
        "`vaire add` wires the dependency it declared, not the whole catalog"
    );
}

// ---- the wiring, through the real binary ------------------------------------

/// A workspace whose members are ready for the CLI: every one has a node and a commit, so
/// `vaire index` has something to build.
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

fn vaire_cli(ws: &Ws, config_home: &Path, pkg: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_vaire"));
    cmd.env("VAIRE_CONFIG_HOME", config_home)
        .env("VAIRE_HOME", ws.home())
        .current_dir(ws.root(pkg))
        .arg("--no-color");
    cmd
}

#[test]
fn index_satisfies_declared_dependencies_from_the_catalog() {
    let ws = cli_workspace();
    let home = tempfile::tempdir().unwrap();

    // The whole wiring step for a fresh clone: record the package once, then `vaire index`.
    let ok = vaire_cli(&ws, home.path(), "acme-web")
        .args(["catalog", "add"])
        .arg(ws.root("know/acme-core"))
        .status()
        .unwrap();
    assert!(ok.success());

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
fn ambient_registration_alone_is_enough_to_resolve_later() {
    let ws = cli_workspace();
    let home = tempfile::tempdir().unwrap();

    // Nobody registers anything by hand: `vaire index` in the dependency records it in
    // passing, which is what keeps the no-ceremony promise after the walk is gone.
    let built = vaire_cli(&ws, home.path(), "know/acme-core")
        .arg("index")
        .status()
        .unwrap();
    assert!(built.success());

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
    assert_eq!(json["dependencies"][0]["status"], "indexed");
}

#[test]
fn add_reports_the_link_the_catalog_made_and_a_read_makes_none() {
    let ws = cli_workspace();
    let home = tempfile::tempdir().unwrap();
    vaire_cli(&ws, home.path(), "acme-web")
        .args(["catalog", "add"])
        .arg(ws.root("know/acme-core"))
        .status()
        .unwrap();

    // `vaire add` declares — and reports the link it could satisfy from the catalog.
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

    // Reads never write links: build, drop the entry, then query — the query must tolerate
    // the missing dependency, not re-link it (and must not touch the catalog's lock).
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

#[test]
fn an_old_local_packages_root_migrates_itself_into_the_catalog_once() {
    let ws = cli_workspace();
    let home = tempfile::tempdir().unwrap();
    // A config written by v0.2.x, with the setting this release retires.
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[packages]\nlocal = \"{}\"\n",
            ws.dir.path().join("know").display()
        ),
    )
    .unwrap();

    let out = vaire_cli(&ws, home.path(), "acme-web")
        .arg("index")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("local-packages is retired"), "{stderr}");

    // The dependency resolved on the same run the migration happened…
    assert!(entry(&ws, "acme-web", "acme-core").exists());
    // …the key is gone…
    let config = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert!(!config.contains("local ="), "the key was dropped: {config}");
    // …and the packages it held are in the catalog for good.
    let listed = vaire_cli(&ws, home.path(), "acme-web")
        .args(["--json", "catalog", "list"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let names: Vec<&str> = json["sightings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"acme-core"), "{names:?}");

    // A second run says nothing: the migration is one-shot, not a per-command walk.
    let again = vaire_cli(&ws, home.path(), "acme-web")
        .arg("index")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(!stderr.contains("local-packages"), "{stderr}");
}

#[test]
fn configure_no_longer_offers_local_packages() {
    let ws = cli_workspace();
    let home = tempfile::tempdir().unwrap();

    let out = vaire_cli(&ws, home.path(), "acme-web")
        .args(["configure", "local-packages", "/tmp"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "the subcommand is gone");
}
