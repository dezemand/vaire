//! The catalog — what this machine knows about packages (cli.md §4.8).
//!
//! Every test drives the real command functions against an explicit home, so nothing here
//! touches the developer's own `~/.vaire` and nothing depends on a process-global
//! environment variable (these run in parallel threads).

mod common;

use std::path::{Path, PathBuf};

use common::Ws;
use vaire::catalog::{Catalog, Origin, State};
use vaire::commands::catalog as cmd;

/// A hermetic directory — used both for the vaire home and for the package trees the
/// tests build, so nothing touches the developer's own `~/.vaire`.
fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A package directory that is not part of any workspace fixture.
fn loose_package(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(
        dir.join("knowledge.toml"),
        format!("name = \"{name}\"\nversion = \"2.1.0\"\n"),
    )
    .expect("manifest");
    dir
}

fn names(home: &Path) -> Vec<String> {
    cmd::list(home)
        .expect("list")
        .sightings
        .into_iter()
        .map(|s| s.name)
        .collect()
}

#[test]
fn add_records_a_package_the_user_named() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");

    let out = cmd::add(home.path(), Some(&pkg)).expect("add");
    assert_eq!(out.recorded.len(), 1, "{out:?}");
    assert_eq!(out.recorded[0].name, "acme-core");
    assert_eq!(out.recorded[0].version, "2.1.0");
    // An explicit registration is a statement of intent, and is recorded as one.
    assert_eq!(out.recorded[0].origin, Origin::Registered);
    assert_eq!(out.recorded[0].state, State::Live);
}

#[test]
fn add_refuses_something_that_is_not_a_package() {
    let home = tmp();
    let work = tmp();
    let err = cmd::add(home.path(), Some(work.path())).expect_err("not a package");
    assert!(err.to_string().contains("knowledge.toml"), "{err}");
}

#[test]
fn recording_the_same_path_twice_keeps_one_row() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");

    cmd::add(home.path(), Some(&pkg)).expect("add");
    cmd::add(home.path(), Some(&pkg)).expect("add again");

    // Sightings are keyed by path, so an observation repeated is still one observation —
    // which is what makes ambient registration safe to run on every command.
    assert_eq!(names(home.path()), ["acme-core"]);
}

#[test]
fn two_paths_declaring_one_name_are_two_sightings() {
    let home = tmp();
    let work = tmp();
    let a = loose_package(work.path(), "acme-core");
    let b = work.path().join("fork");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(
        b.join("knowledge.toml"),
        "name = \"acme-core\"\nversion = \"3.0.0\"\n",
    )
    .unwrap();

    cmd::add(home.path(), Some(&a)).expect("add");
    cmd::add(home.path(), Some(&b)).expect("add fork");

    // A fork beside its original is genuine ambiguity, and the catalog's job is to *record*
    // it rather than pick. Which one wins is a resolution question, answered later.
    let catalog = Catalog::open(home.path()).expect("open");
    let rows = catalog.by_name("acme-core").expect("by_name");
    assert_eq!(rows.len(), 2, "{rows:?}");
}

#[test]
fn scan_imports_a_tree_at_any_depth() {
    let home = tmp();
    let work = tmp();
    loose_package(work.path(), "acme-core");
    // Nested inside a bigger repository, with no directory named after the package —
    // matching is by declared name, never by path.
    let nested = work.path().join("platform-docs/docs/kb");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        nested.join("knowledge.toml"),
        "name = \"acme-handbook\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    let out = cmd::scan_dir(home.path(), work.path()).expect("scan");
    assert_eq!(out.recorded.len(), 2, "{out:?}");
    let mut found = names(home.path());
    found.sort();
    assert_eq!(found, ["acme-core", "acme-handbook"]);
    assert!(
        out.recorded.iter().all(|s| s.origin == Origin::Scanned),
        "a bulk import is recorded as such: {out:?}"
    );
}

#[test]
fn scan_reports_an_unreadable_manifest_instead_of_hiding_it() {
    let home = tmp();
    let work = tmp();
    let broken = work.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("knowledge.toml"), "name = \"1-bad-slug\"\n").unwrap();

    let out = cmd::scan_dir(home.path(), work.path()).expect("scan");
    assert!(out.recorded.is_empty(), "{out:?}");
    // A malformed manifest must not look like an absent package.
    assert_eq!(out.unreadable.len(), 1, "{out:?}");
}

#[test]
fn a_vanished_path_becomes_missing_and_only_then_can_be_swept() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");
    cmd::add(home.path(), Some(&pkg)).expect("add");

    std::fs::remove_dir_all(&pkg).expect("remove the package");

    // Listing re-checks: no timers, no expiry — a path is missing because someone looked.
    let listed = cmd::list(home.path()).expect("list");
    assert_eq!(listed.sightings.len(), 1, "the row is kept, not deleted");
    assert_eq!(listed.sightings[0].state, State::Missing);

    let out = cmd::remove(home.path(), None, true).expect("rm --missing");
    assert_eq!(out.removed, 1);
    assert!(names(home.path()).is_empty());
}

#[test]
fn a_returning_path_goes_live_again() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");
    cmd::add(home.path(), Some(&pkg)).expect("add");
    std::fs::remove_dir_all(&pkg).expect("unmount");
    assert_eq!(
        cmd::list(home.path()).unwrap().sightings[0].state,
        State::Missing
    );

    // An unmounted drive comes back; a restored checkout comes back. Nothing was lost in
    // between, which is the whole reason missing is a state and not a deletion.
    loose_package(work.path(), "acme-core");
    let listed = cmd::list(home.path()).expect("list");
    assert_eq!(listed.sightings[0].state, State::Live);
}

#[test]
fn rm_takes_a_path_or_a_name() {
    let home = tmp();
    let work = tmp();
    let a = loose_package(work.path(), "acme-core");
    loose_package(work.path(), "acme-web");

    cmd::add(home.path(), Some(&a)).expect("add");
    cmd::add(home.path(), Some(&work.path().join("acme-web"))).expect("add");

    let by_path = cmd::remove(home.path(), Some(&a.display().to_string()), false).expect("rm path");
    assert_eq!(by_path.removed, 1);
    assert_eq!(names(home.path()), ["acme-web"]);

    let by_name = cmd::remove(home.path(), Some("acme-web"), false).expect("rm name");
    assert_eq!(by_name.removed, 1);
    assert!(names(home.path()).is_empty());
}

#[test]
fn rm_without_a_target_says_what_it_needs() {
    let home = tmp();
    let err = cmd::remove(home.path(), None, false).expect_err("needs a target");
    assert!(err.to_string().contains("--missing"), "{err}");
}

// ---- ambient registration --------------------------------------------------

#[test]
fn a_maintain_command_records_the_package_and_its_closure() {
    let home = tmp();
    let ws = Ws::acceptance();

    // One `vaire index` in a consumer is enough for the catalog to know every working copy
    // that run reached — which is what keeps registration from becoming ceremony.
    let warnings = cmd::register_ambient_in(home.path(), &ws.ctx("acme-web"), false);
    assert!(warnings.is_empty(), "{warnings:?}");

    let mut found = names(home.path());
    found.sort();
    assert_eq!(found, ["acme-core", "acme-shared", "acme-web"]);
    let catalog = Catalog::open(home.path()).expect("open");
    assert_eq!(
        catalog.by_name("acme-web").unwrap()[0].origin,
        Origin::Ambient
    );
}

#[test]
fn no_register_skips_but_never_forgets() {
    let home = tmp();
    let ws = Ws::acceptance();
    let root = ws.root("acme-web");

    cmd::add(home.path(), Some(&root)).expect("add");
    let warnings = cmd::register_ambient_in(home.path(), &ws.ctx("acme-web"), true);
    assert!(warnings.is_empty());

    // Skipping is not forgetting: the row the user asked for is exactly as it was, and no
    // closure member was silently added behind the flag.
    assert_eq!(names(home.path()), ["acme-web"]);
    let catalog = Catalog::open(home.path()).expect("open");
    assert_eq!(
        catalog.by_name("acme-web").unwrap()[0].origin,
        Origin::Registered
    );
}

#[test]
fn an_ambient_touch_does_not_demote_an_explicit_registration() {
    let home = tmp();
    let ws = Ws::acceptance();
    let root = ws.root("acme-web");

    cmd::add(home.path(), Some(&root)).expect("add");
    cmd::register_ambient_in(home.path(), &ws.ctx("acme-web"), false);

    // "I registered this" outranks "something happened to touch it", so a later ambient
    // observation refreshes the row without rewriting why it is there.
    let catalog = Catalog::open(home.path()).expect("open");
    let rows = catalog.by_name("acme-web").expect("by_name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].origin, Origin::Registered);
}

#[test]
fn a_broken_catalog_never_fails_the_command_that_touched_it() {
    let home = tmp();
    let ws = Ws::acceptance();
    // A home that cannot hold a catalog at all (a file where the directory should be).
    let blocked = home.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("write");

    let warnings = cmd::register_ambient_in(&blocked, &ws.ctx("acme-web"), false);

    // The catalog is a convenience over state that can be rebuilt by scanning. Losing it
    // must degrade `vaire index`, never stop it.
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("catalog not updated"), "{warnings:?}");
}

#[test]
fn a_catalog_from_an_older_unmigratable_schema_is_recreated_not_relabelled() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");
    {
        let catalog = Catalog::open(home.path()).expect("catalog");
        catalog
            .record(&pkg, "acme-core", "1.0.0", Origin::Registered)
            .expect("record");
        // An older shape with no migration step to bring it forward.
        catalog
            .set_schema_version(0)
            .expect("stamp an older version");
    }

    // The tables are created `IF NOT EXISTS`, so installing over an older shape would
    // no-op and then stamp it as current — leaving a database whose columns no query
    // matches, now labelled as though it did. Starting again is the only honest answer.
    let catalog = Catalog::open(home.path()).expect("open");
    assert_eq!(
        catalog.schema_version().unwrap(),
        Some(vaire::catalog::SCHEMA_VERSION)
    );
    assert!(
        catalog.sightings().expect("readable").is_empty(),
        "a recreated catalog starts empty; a rescan refills it"
    );
    // Even here nothing is destroyed: what could not be read is kept beside the new file,
    // because a pin and a pulled-by-name record are not observations a rescan reproduces.
    assert!(
        home.path().join("catalog.db.unreadable").is_file(),
        "the displaced catalog is kept"
    );
}

#[test]
fn a_vanished_path_can_still_be_removed_by_path() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");
    cmd::add(home.path(), Some(&pkg)).expect("add");
    std::fs::remove_dir_all(&pkg).expect("the checkout goes away");

    // This is the case people actually reach for `rm <path>` in. Deciding path-vs-name by
    // whether the path still exists would send it down the name branch, match nothing, and
    // report success having done nothing.
    let out = cmd::remove(home.path(), Some(&pkg.display().to_string()), false).expect("rm");
    assert_eq!(out.removed, 1, "{out:?}");
    assert!(names(home.path()).is_empty());
}

#[test]
fn sweeping_a_clean_catalog_says_so_rather_than_reporting_a_failed_match() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");
    cmd::add(home.path(), Some(&pkg)).expect("add");

    // A clean catalog reaches this every time the sweep runs, so it must not read like a
    // lookup that found nothing.
    let out = cmd::remove(home.path(), None, true).expect("sweep");
    assert_eq!(out.removed, 0);
    assert!(out.swept);
    let rendered = vaire::output::Output::render_human(&out);
    assert!(rendered.contains("still answers"), "{rendered}");
}

// ---- schema v2 ---------------------------------------------------------------------------

#[test]
fn a_v1_catalog_is_migrated_rather_than_recreated() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");
    cmd::add(home.path(), Some(&pkg)).expect("add");
    {
        // A catalog as v1 left it. Recreating instead of migrating used to cost a rescan;
        // it now costs *deletions*, because `vaire clean` reads its roots from exactly these
        // rows — a machine that forgot every workspace would sweep away the releases their
        // lockfiles were holding.
        let catalog = Catalog::open(home.path()).expect("open");
        catalog.set_schema_version(1).expect("pretend to be v1");
    }

    let catalog = Catalog::open(home.path()).expect("reopen");
    assert_eq!(catalog.schema_version().expect("version"), Some(2));
    assert_eq!(
        catalog.sightings().expect("sightings").len(),
        1,
        "the registration survived the upgrade"
    );
}

#[test]
fn a_half_finished_migration_finishes_on_the_next_open() {
    let home = tmp();
    let work = tmp();
    let pkg = loose_package(work.path(), "acme-core");
    cmd::add(home.path(), Some(&pkg)).expect("add");
    {
        // Killed between the schema change and the version stamp — the tables are already
        // v2 and the recorded version is not. There is no transaction spanning the two, so
        // the next open has to be able to finish rather than fail on the column it finds
        // already there.
        let catalog = Catalog::open(home.path()).expect("open");
        catalog.set_schema_version(1).expect("stamp");
    }

    let catalog = Catalog::open(home.path()).expect("reopen");
    assert_eq!(catalog.schema_version().expect("version"), Some(2));
    assert_eq!(catalog.sightings().expect("sightings").len(), 1);
}

#[test]
fn a_standing_request_carries_onto_a_replacement_version() {
    let home = tmp();
    let catalog = Catalog::open(home.path()).expect("open");
    let entry = |version: &str, requested: bool| vaire::catalog::StoreEntry {
        name: "acme-core".into(),
        version: version.parse().expect("version"),
        registry: Some("lab".into()),
        pinned: false,
        requested,
        last_used: None,
    };
    catalog.record_release(&entry("1.0.0", true)).expect("pull");

    // A later manifest-driven pull of the next version. The request was made about the
    // *package*, so retention taking 1.0.0 away must not retract it along with the bytes.
    catalog
        .record_release(&entry("1.1.0", false))
        .expect("pull");
    let releases = catalog.releases().expect("releases");
    assert!(releases.iter().all(|entry| entry.requested), "{releases:?}");

    // An unrelated package is untouched by any of it.
    catalog
        .record_release(&vaire::catalog::StoreEntry {
            name: "acme-other".into(),
            version: "1.0.0".parse().expect("version"),
            registry: None,
            pinned: false,
            requested: false,
            last_used: None,
        })
        .expect("pull");
    let other = catalog
        .releases()
        .expect("releases")
        .into_iter()
        .find(|entry| entry.name == "acme-other")
        .expect("recorded");
    assert!(!other.requested);
}

/// A store entry is a package directory, so nothing about the *path* refuses it — which
/// is exactly why the command has to (registry.md §4.3).
///
/// A sighting claims "observed at a path, and may have changed since". A sealed release is
/// the opposite claim, and recording one would make a single directory arrive under two
/// identities — the second of them outranking the store in resolution, as a working copy
/// somebody edits.
#[test]
fn catalog_add_refuses_a_release_in_the_store() {
    let home = tmp();
    let entry = home.path().join("store/acme-core/1.4.2");
    std::fs::create_dir_all(&entry).expect("entry");
    std::fs::write(
        entry.join("knowledge.toml"),
        "name = \"acme-core\"\nversion = \"1.4.2\"\n",
    )
    .expect("manifest");

    let refused =
        cmd::add(home.path(), Some(&entry)).expect_err("a store entry is not a workspace");
    let message = refused.to_string();
    assert!(
        message.contains("release in the store"),
        "the refusal says what it found: {message}"
    );
    assert!(names(home.path()).is_empty(), "and nothing was recorded");
}

/// A scan is skipped rather than refused, because pointing one at a directory that happens
/// to contain the vaire home is an ordinary thing to do — `vaire catalog scan ~`. Failing
/// the whole import over a store entry would be useless; importing it would be wrong.
#[test]
fn catalog_scan_walks_past_the_store_and_still_imports_the_rest() {
    let home = tmp();
    let entry = home.path().join("store/acme-core/1.4.2");
    std::fs::create_dir_all(&entry).expect("entry");
    std::fs::write(
        entry.join("knowledge.toml"),
        "name = \"acme-core\"\nversion = \"1.4.2\"\n",
    )
    .expect("manifest");
    // A real working copy in the same tree, so the test proves the scan still works.
    loose_package(home.path(), "acme-glossary");

    cmd::scan_dir(home.path(), home.path()).expect("scan");
    assert_eq!(
        names(home.path()),
        vec!["acme-glossary".to_string()],
        "the working copy is imported and the sealed release is not"
    );
}
