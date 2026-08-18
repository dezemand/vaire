//! Does Turso hold up as **cross-process** state?
//!
//! The catalog is the first thing Vairë writes from more than one process at a time —
//! parallel agent sessions, a CI job beside an editor, a `vaire index` in one terminal
//! while another registers a clone. Every other database in the system (a package's index)
//! is effectively single-writer, so this is a genuinely new demand and the design leaned
//! on an untested assumption: that short, idempotent, single-statement transactions
//! against a WAL database are safe under real concurrency.
//!
//! These tests spawn actual OS processes rather than threads. Threads would share one
//! Turso instance and its internal locking, which is exactly the thing not under test —
//! the question is whether *separate processes* holding separate connections to the same
//! file can interleave writes without losing or corrupting rows.
//!
//! ## What it found (2026-08-11), and why the design changed
//!
//! **The assumption was wrong, and more sharply than expected.** Turso takes an exclusive
//! lock when a database is *opened*: six of eight processes did not fail to write, they
//! failed to **open** — `Locking error: Failed locking file 'catalog.db'. File is locked
//! by another process`. Reads are no exception; there is no shared mode to fall back to.
//!
//! It also caught a bug the naive reading would have shipped: an `open` that treated every
//! connect failure as corruption would have *deleted the catalog* whenever a colleague's
//! `vaire index` happened to be holding it.
//!
//! The resolution keeps Turso, because the exclusive lock **is** the cross-process mutex
//! that would otherwise have needed building — and an OS file lock is released when its
//! process dies, so there are no stale locks to recover from. Two rules make it work, both
//! enforced in `catalog`: connections are short-lived, and contention is retried rather
//! than failed. With those, this test passes with every write landing exactly once. The
//! cost is honest and worth restating: **catalog access across processes is serialized**,
//! so anything holding a catalog handle open across slow work blocks every other vaire
//! process on the machine.

use std::path::{Path, PathBuf};
use std::process::Command;

use vaire::catalog::{Catalog, Origin, State};

/// Processes to run at once. Above any plausible number of concurrent vaire invocations
/// on one machine, which is the point — the contention here is worse than reality's.
const WORKERS: usize = 8;
/// Distinct packages each worker registers.
const PER_WORKER: usize = 25;

/// The environment variable that turns this test binary into a worker process. Absent in a
/// normal run, so the worker test below is a no-op unless a parent asked for it.
const WORKER_ENV: &str = "VAIRE_CATALOG_WORKER";

/// A package directory the catalog will accept (it canonicalizes paths, so these must
/// really exist).
fn package_at(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("package dir");
    std::fs::write(
        dir.join("knowledge.toml"),
        format!("name = \"{name}\"\nversion = \"1.0.0\"\n"),
    )
    .expect("manifest");
    dir
}

/// The worker half: open the shared catalog and hammer it. Invoked only as a child
/// process — a normal `cargo test` run sees the variable unset and returns immediately.
#[test]
fn catalog_write_worker() {
    let Ok(spec) = std::env::var(WORKER_ENV) else {
        return;
    };
    let (home, id) = spec.split_once('|').expect("worker spec is <home>|<id>");
    let home = PathBuf::from(home);
    let root = home.join("packages");

    let catalog = Catalog::open(&home).expect("worker opens the shared catalog");
    // The contended directory is created once by the parent. Eight processes rewriting one
    // `knowledge.toml` is not an atomic operation, and a torn read of it would flake this
    // test for a reason that has nothing to do with catalog concurrency — the contended
    // *row* is the point, not the file behind it.
    let shared = root.join("contended");
    for n in 0..PER_WORKER {
        let name = format!("pkg-{id}-{n}");
        let dir = package_at(&root, &name);
        catalog
            .record(&dir, &name, "1.0.0", Origin::Ambient)
            .expect("worker records its own package");

        // Every worker also writes the *same* row, so the upsert path is genuinely
        // contended rather than merely concurrent.
        catalog
            .record(&shared, "contended", "1.0.0", Origin::Ambient)
            .expect("worker records the contended package");
    }
}

#[test]
fn eight_processes_writing_at_once_lose_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    // Create the catalog up front so every worker opens an existing file — the realistic
    // case, and it keeps the test measuring writes rather than eight racing creations.
    drop(Catalog::open(&home).expect("catalog"));
    // …and the contended package, so no worker has to write the file every other worker
    // is reading.
    package_at(&home.join("packages"), "contended");

    let exe = std::env::current_exe().expect("test binary path");
    let started = std::time::Instant::now();
    let children: Vec<_> = (0..WORKERS)
        .map(|id| {
            Command::new(&exe)
                .args(["catalog_write_worker", "--exact", "--quiet"])
                .env(WORKER_ENV, format!("{}|{id}", home.display()))
                .spawn()
                .expect("spawn worker")
        })
        .collect();

    let mut failures = Vec::new();
    for (id, mut child) in children.into_iter().enumerate() {
        let status = child.wait().expect("worker exits");
        if !status.success() {
            failures.push(format!("worker {id}: {status}"));
        }
    }
    let elapsed = started.elapsed();
    assert!(failures.is_empty(), "workers failed: {failures:?}");

    let catalog = Catalog::open(&home).expect("catalog reopens after the storm");
    let sightings = catalog.sightings().expect("sightings readable");

    // Every distinct package each worker registered must be present exactly once.
    let expected = WORKERS * PER_WORKER + 1; // + the contended row
    assert_eq!(
        sightings.len(),
        expected,
        "expected {expected} rows, got {} — a lost or duplicated write",
        sightings.len()
    );
    for id in 0..WORKERS {
        for n in 0..PER_WORKER {
            let name = format!("pkg-{id}-{n}");
            let rows = catalog.by_name(&name).expect("lookup");
            assert_eq!(rows.len(), 1, "{name} should have exactly one sighting");
        }
    }
    // The contended row: eight processes wrote it 25 times each, and it is still one row.
    let contended = catalog.by_name("contended").expect("lookup");
    assert_eq!(
        contended.len(),
        1,
        "the contended path must converge on one row, not {}",
        contended.len()
    );
    assert_eq!(contended[0].state, State::Live);

    // Recorded for the record — the design leaned on this being fast enough to do on
    // every maintain command, not merely correct.
    println!(
        "catalog concurrency: {WORKERS} processes × {PER_WORKER} packages \
         (+{} contended writes) = {} writes in {:?}",
        WORKERS * PER_WORKER,
        WORKERS * PER_WORKER * 2,
        elapsed
    );
}

#[test]
fn a_corrupt_catalog_is_recreated_rather_than_repaired() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let pkg = package_at(&home, "acme-core");
    {
        let catalog = Catalog::open(&home).expect("catalog");
        catalog
            .record(&pkg, "acme-core", "1.0.0", Origin::Registered)
            .expect("record");
    }
    // Every row is an observation a rescan can produce again, so the cheapest correct
    // answer to an unreadable catalog is to throw it away — never to fail a command.
    std::fs::write(home.join("catalog.db"), b"this is not a database").expect("corrupt it");

    let catalog = Catalog::open(&home).expect("a corrupt catalog opens by being recreated");
    assert_eq!(
        catalog.schema_version().unwrap(),
        Some(vaire::catalog::SCHEMA_VERSION)
    );
    assert!(
        catalog.sightings().expect("readable").is_empty(),
        "the recreated catalog starts empty — rescanning is what refills it"
    );
    // Rebuilt, but not thrown away: what could not be read is kept beside the new file.
    // A pin and a pulled-by-name record are the only note anywhere that a stored release
    // is spoken for, so the judgement "this is garbage" must stay reversible.
    assert!(
        home.join("catalog.db.unreadable").is_file(),
        "the displaced catalog is kept beside the new one"
    );
}

/// A catalog this process cannot **open** is a fault of the machine, not of the file.
///
/// The distinction matters because the two look identical from the engine: both arrive as
/// "I/O error". Deleting on the strength of that would mean a home directory with the
/// wrong permissions — a restored backup, a `sudo` that got in somewhere — silently
/// costing every pin and pulled-by-name record on the machine, and `vaire clean` then
/// deleting the releases those were holding.
#[test]
fn an_unreachable_catalog_is_an_error_and_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let pkg = package_at(&home, "acme-core");
    {
        let catalog = Catalog::open(&home).expect("catalog");
        catalog
            .record(&pkg, "acme-core", "1.0.0", Origin::Registered)
            .expect("record");
    }
    let db = home.join("catalog.db");
    let before = std::fs::read(&db).expect("readable before");
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o000)).expect("chmod");

    let opened = Catalog::open(&home);
    // Running as root defeats the premise rather than the fix; skip instead of asserting
    // something the environment cannot demonstrate.
    if opened.is_ok() {
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).expect("restore");
        eprintln!("skipped: this user can read a 000 file");
        return;
    }
    let message = opened.err().expect("refused").to_string();
    assert!(
        message.contains("left exactly as it is"),
        "the refusal says the file was not touched: {message}"
    );

    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).expect("restore");
    assert_eq!(
        std::fs::read(&db).expect("still there"),
        before,
        "an unreachable catalog is left byte-for-byte as it was"
    );
    assert!(
        !home.join("catalog.db.unreadable").exists(),
        "nothing was displaced either — there was nothing wrong with the file"
    );
    let catalog = Catalog::open(&home).expect("readable again once permissions allow");
    assert_eq!(
        catalog.sightings().expect("readable").len(),
        1,
        "the sighting survived the refusal"
    );
}

/// A catalog from a newer vaire is refused, exactly as `knowledge.lock` is.
///
/// Refusing to *read* a format you do not know is only coherent if you also refuse to
/// *overwrite* it. Rebuilding here would forget which workspaces and pins hold the store's
/// releases — which is what the next `vaire clean` consults before deleting them.
#[test]
fn a_catalog_from_a_newer_vaire_is_refused_rather_than_rebuilt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let pkg = package_at(&home, "acme-core");
    {
        let catalog = Catalog::open(&home).expect("catalog");
        catalog
            .record(&pkg, "acme-core", "1.0.0", Origin::Registered)
            .expect("record");
    }
    {
        let db = vaire::db::Db::connect(&home.join("catalog.db")).expect("raw handle");
        db.execute("DELETE FROM schema_version", ()).expect("clear");
        db.execute(
            "INSERT INTO schema_version(version) VALUES(?1)",
            [i64::from(vaire::catalog::SCHEMA_VERSION + 1)],
        )
        .expect("stamp a newer schema");
    }

    let message = Catalog::open(&home)
        .err()
        .expect("a newer catalog is refused")
        .to_string();
    assert!(
        message.contains("newer vaire") && message.contains("vaire upgrade"),
        "the refusal names the cause and the way out: {message}"
    );
    assert!(
        !home.join("catalog.db.unreadable").exists(),
        "a refusal displaces nothing"
    );

    // And the rows are still there for the vaire that can read them.
    let db = vaire::db::Db::connect(&home.join("catalog.db")).expect("raw handle");
    let rows: Vec<i64> = db
        .query_rows("SELECT COUNT(*) FROM workspaces", (), |row| {
            Ok(row.get_value(0)?.as_integer().copied().unwrap_or(0))
        })
        .expect("count");
    assert_eq!(
        rows.first().copied(),
        Some(1),
        "the sighting a newer vaire recorded is untouched"
    );
}
