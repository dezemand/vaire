//! The catalog — what packages this machine knows, where they live, and whether they may
//! be edited (registry.v2.md §4).
//!
//! One object answers those questions; scans, registrations, and received artifacts all
//! *feed* it, and resolution consults only it. It is an **inventory and a mediator, never
//! an authority**: the corpus is still the truth, every per-package index still lives
//! beside its package, and nothing here ever holds entity content — only package-level
//! metadata.
//!
//! ## An index, never truth
//!
//! Every row is an **observation**, so the catalog is always rebuildable and never
//! believed on its own. A sighting says "a package declaring *N* at version *V* was seen
//! at path *P*"; before anything is resolved from one, the manifest is re-read and must
//! still say the same thing. That is why losing this file costs nothing but a rescan, and
//! why a stale row heals instead of misleading.
//!
//! ## Two states, no clocks
//!
//! A sighting is `live` or `missing` — nothing expires on a timer, and nothing is ever
//! removed behind the user's back. Checking is cheap enough (one `lstat`) to do on every
//! consult, so freshness is *observed* rather than *assumed to decay*: a path that goes
//! away is marked `missing` and excluded from resolution; the same path reappearing —
//! a remounted drive, a restored checkout — flips straight back to `live`. Removal is
//! always explicit (`vaire catalog rm`).
//!
//! ## Shared across processes — and what that actually costs
//!
//! This is the first Vairë state used by more than one process at a time: parallel agent
//! sessions, a CI job beside an editor. The original design assumed short WAL transactions
//! would make that safe. **They do not, and the reason is sharper than write contention:
//! Turso takes an exclusive lock when a database is *opened*, so a second process cannot
//! open the catalog at all — not even to read it — while a first one holds it.** A
//! multi-process test proved it before any of this was relied upon
//! (`tests/catalog_concurrency.rs`).
//!
//! That turns out to be workable, because the lock *is* the mutex we would otherwise have
//! had to build. Two rules follow, and both are load-bearing:
//!
//! * **Connections are short-lived.** Open, do the one thing, drop. A handle held across
//!   anything slow — a corpus walk, an index build, an HTTP request — locks every other
//!   vaire process on the machine out of the catalog for that whole time.
//! * **Contention is retried, never failed** ([`Catalog::open`]). Waiting is correct here:
//!   the holder is milliseconds away from finishing, and an OS file lock is released when
//!   its process dies, so there is no such thing as a stale catalog lock.
//!
//! Writes stay idempotent upserts regardless, so the worst case of a race remains that two
//! processes record the same observation twice.

pub mod scan;

use std::path::{Path, PathBuf};

use crate::db::{Db, col_i64, col_opt_i64, col_opt_text, col_text};
use crate::error::{Result, VaireError};

/// The catalog's schema version, stamped in its own `schema_version` table — the same
/// discipline the package index uses. **Bump on any schema change**; a catalog written by
/// a newer vaire is recreated rather than misread, which costs a rescan and nothing else.
/// v2 adds `releases.requested` — see the table below.
///
/// A version [`Catalog::migrate`] knows is brought forward in place; anything else is still
/// recreated, which stays affordable because every row is an observation something can
/// produce again (`vaire catalog scan`, and a walk of the store). What made a migration
/// worth writing for this one bump is that `vaire clean` reads its roots from these rows, so
/// forgetting them stopped costing a rescan and started costing deletions.
pub const SCHEMA_VERSION: u32 = 2;

/// All four tables ship in v1, even where this milestone has no writer for them yet
/// (registries, releases, and provenance arrive with the registry client and the store).
/// Their shapes are already decided, and a schema bump per table would mean recreating
/// the catalog three more times for no gain.
const SCHEMA_STMTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)",
    // A sighting: a package observed at a path. Keyed by the **canonicalized** path, so
    // two routes to one directory cannot register twice and raise a false ambiguity.
    // `name` is an attribute, never the key — identity is declared, and which sighting is
    // "the" package is a resolution-time question, never a write-time guess.
    "CREATE TABLE IF NOT EXISTS workspaces (
        path      TEXT PRIMARY KEY,
        name      TEXT NOT NULL,
        version   TEXT NOT NULL,     -- at last sighting; re-read before it is believed
        state     TEXT NOT NULL,     -- 'live' | 'missing'
        origin    TEXT NOT NULL,     -- 'registered' | 'scanned' | 'ambient'
        last_seen INTEGER            -- epoch seconds of the last successful validation
    )",
    "CREATE INDEX IF NOT EXISTS workspaces_name ON workspaces(name)",
    // Remotes. Populated by `vaire registry add` when the registry client lands.
    "CREATE TABLE IF NOT EXISTS registries (
        name              TEXT PRIMARY KEY,
        url               TEXT NOT NULL,
        kind              TEXT NOT NULL,
        priority          INTEGER NOT NULL DEFAULT 0,
        search_by_default INTEGER NOT NULL DEFAULT 1
    )",
    // Materialized releases in the store. Populated by `vaire pull`.
    //
    // `requested` is what separates a package somebody asked for by name from one that
    // arrived because a manifest mentioned it, and it exists for `vaire clean`: a
    // dependency is rooted by the lockfile that records it, and a package pulled to *read*
    // — the rootless session's whole corpus — is recorded by no lockfile at all. Without
    // this column the two are indistinguishable in the store, and a sweep that keeps only
    // what a lockfile names would delete a reader's library.
    "CREATE TABLE IF NOT EXISTS releases (
        name      TEXT NOT NULL,
        version   TEXT NOT NULL,
        origin    TEXT NOT NULL,     -- 'store' | 'remote'
        registry  TEXT,
        pinned    INTEGER NOT NULL DEFAULT 0,
        requested INTEGER NOT NULL DEFAULT 0,
        last_used INTEGER,
        PRIMARY KEY (name, version)
    )",
    // The ref cache: which registry last answered for a name. Advisory routing only.
    "CREATE TABLE IF NOT EXISTS provenance (
        name           TEXT NOT NULL,
        registry       TEXT NOT NULL,
        first_seen     INTEGER,
        last_confirmed INTEGER,
        PRIMARY KEY (name, registry)
    )",
];

/// Whether a sighting's path still answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Live,
    Missing,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Live => "live",
            State::Missing => "missing",
        }
    }

    fn parse(s: &str) -> State {
        match s {
            "missing" => State::Missing,
            _ => State::Live,
        }
    }
}

/// How a sighting came to be recorded. Ordering is meaningful for later selection:
/// an explicit registration outranks something a scan or a passing command noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// `vaire catalog add` — the user said so.
    Registered,
    /// `vaire catalog scan` — a bulk import walked it up.
    Scanned,
    /// A maintain command touched this package and recorded it in passing.
    Ambient,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Registered => "registered",
            Origin::Scanned => "scanned",
            Origin::Ambient => "ambient",
        }
    }

    fn parse(s: &str) -> Origin {
        match s {
            "registered" => Origin::Registered,
            "scanned" => Origin::Scanned,
            _ => Origin::Ambient,
        }
    }
}

/// A configured remote registry (registry.v2.md §8, §12).
///
/// The one row in this catalog that is **not** an observation: nobody stumbles across a
/// registry, someone configures it. That is why there is no state machine here and no
/// ambient registration — a registry is present because it was named, and it leaves when it
/// is removed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RegistryRow {
    /// What this machine calls it. Local: two people may know one bucket by two names.
    pub name: String,
    pub url: String,
    /// Which client speaks to it — `static` today, `api` when the mediating service exists.
    pub kind: String,
    /// Fan-out order, and the tiebreaker when a command that needs *one* registry is not
    /// told which. Higher first.
    pub priority: i64,
    /// Whether `vaire search` reaches this registry without being asked to (§10). No reader
    /// yet — the fan-out engine arrives with the store — but the column is written now so
    /// the setting does not have to be re-collected later.
    pub search_by_default: bool,
}

/// The registry kinds this client can construct.
pub const KIND_STATIC: &str = "static";

/// A materialized release in the store (registry.v2.md §5).
///
/// Unlike a sighting, this is not an observation of something that might drift: a store
/// entry is written once and sealed. The row is an index over the entry's own
/// `source.toml`, kept so that "what has this machine got" is a query rather than a walk —
/// and rebuildable from that walk when it is lost.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StoreEntry {
    pub name: String,
    pub version: crate::model::Version,
    /// Which configured registry it came from, when it is still known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Held against retention and `vaire clean`. Written by `vaire pin`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// Somebody asked for this package **by name** rather than declaring it. A root for
    /// `vaire clean` in its own right, because nothing else records it: a named pull from
    /// outside a package writes no lockfile, and that is exactly how a rootless reader
    /// builds a corpus.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
}

/// One observation of a package on this machine.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Sighting {
    pub path: PathBuf,
    pub name: String,
    pub version: String,
    pub state: State,
    pub origin: Origin,
    /// Epoch seconds of the last successful validation, if it has ever been validated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<i64>,
}

/// The open catalog.
pub struct Catalog {
    db: Db,
    path: PathBuf,
}

impl Catalog {
    /// Open the catalog in `home`, creating it if absent, **waiting out** another process
    /// that currently holds it.
    ///
    /// Two failure modes, and telling them apart is the whole job here:
    ///
    /// * **Locked** — another vaire process has it open. Retried on a short poll up to
    ///   [`LOCK_WAIT`], because the holder is milliseconds from finishing and an OS lock
    ///   cannot outlive its process.
    /// * **Unreadable** — corrupt, or written by a newer vaire. Then the file is
    ///   **recreated rather than repaired**: every row is an observation a rescan can
    ///   produce again, which is what lets this state be a cache.
    ///
    /// Conflating the two would be catastrophic in the ordinary case: deleting the catalog
    /// because a colleague's `vaire index` happened to be holding it.
    pub fn open(home: &Path) -> Result<Catalog> {
        std::fs::create_dir_all(home)?;
        let path = home.join("catalog.db");
        let started = std::time::Instant::now();
        let mut attempt = 0u32;
        loop {
            // Whether the file was already there decides what a version mismatch *means*,
            // so it must be answered before connecting creates one.
            let existed = path.exists();
            match Catalog::connect(&path) {
                Ok(catalog) => {
                    if !existed {
                        catalog.install_schema()?;
                        return Ok(catalog);
                    }
                    let found = catalog.schema_version()?;
                    if found == Some(SCHEMA_VERSION) {
                        return Ok(catalog);
                    }
                    // A version this build knows how to bring forward: do that instead of
                    // recreating. See [`Catalog::migrate`] for why one bump earns the
                    // machinery the rest of this file deliberately does without.
                    if let Some(found) = found
                        && catalog.migrate(found)?
                    {
                        return Ok(catalog);
                    }
                    // A catalog from a different vaire. Installing over it would be the
                    // worst outcome available: the `CREATE TABLE IF NOT EXISTS` statements
                    // no-op against the old tables, the version row is then stamped as
                    // current, and every later query fails against columns that were never
                    // migrated. Recreate instead — dropping the handle first, because the
                    // file is still locked by it.
                    drop(catalog);
                    remove_db_files(&path)?;
                    let catalog = Catalog::connect(&path)?;
                    catalog.install_schema()?;
                    return Ok(catalog);
                }
                Err(e) if is_locked(&e) && started.elapsed() < LOCK_WAIT => {
                    attempt += 1;
                    std::thread::sleep(backoff(attempt));
                }
                Err(e) if is_locked(&e) => {
                    return Err(VaireError::Config(format!(
                        "the catalog at {} is held by another vaire process and did not \
                         free up within {}s — if nothing else is running, remove it and it \
                         will be rebuilt by `vaire catalog scan`",
                        path.display(),
                        LOCK_WAIT.as_secs()
                    )));
                }
                Err(_) => {
                    // Unreadable rather than busy: throw it away and start again.
                    remove_db_files(&path)?;
                    let catalog = Catalog::connect(&path)?;
                    catalog.install_schema()?;
                    return Ok(catalog);
                } // no other arms: `is_locked` splits every remaining error above.
            }
        }
    }

    /// Open the catalog in the default vaire home ([`crate::userconfig::vaire_home`]).
    pub fn open_default() -> Result<Catalog> {
        Catalog::open(&crate::userconfig::vaire_home())
    }

    fn connect(path: &Path) -> Result<Catalog> {
        Ok(Catalog {
            db: Db::connect(path)?,
            path: path.to_path_buf(),
        })
    }

    /// Install the schema into a **fresh** file and stamp its version.
    ///
    /// Only ever called on a database known to have no tables — a new one, or one
    /// [`Catalog::open`] has just recreated. It must not be used to "upgrade" an existing
    /// catalog: the statements are `IF NOT EXISTS`, so against an older shape they would
    /// silently no-op and then stamp the old tables as current.
    fn install_schema(&self) -> Result<()> {
        for stmt in SCHEMA_STMTS {
            self.db.execute(stmt, ())?;
        }
        self.db.execute("DELETE FROM schema_version", ())?;
        self.db.execute(
            "INSERT INTO schema_version(version) VALUES(?1)",
            [i64::from(SCHEMA_VERSION)],
        )?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> Result<Option<u32>> {
        Ok(self
            .db
            .query_opt("SELECT version FROM schema_version LIMIT 1", (), |r| {
                col_i64(r, 0)
            })
            .unwrap_or(None)
            .map(|v| v as u32))
    }

    /// Overwrite the stamped schema version. A test seam, mirroring the one on the package
    /// index: it is the only way to model a catalog written by a different vaire.
    pub fn set_schema_version(&self, version: u32) -> Result<()> {
        self.db.execute("DELETE FROM schema_version", ())?;
        self.db.execute(
            "INSERT INTO schema_version(version) VALUES(?1)",
            [i64::from(version)],
        )?;
        Ok(())
    }

    /// Record (or refresh) a sighting. Idempotent by path, which is what makes it safe
    /// under concurrent writers: two processes observing the same package converge on one
    /// row rather than racing to duplicate it.
    ///
    /// `path` is canonicalized here, so callers may pass any route to the directory.
    pub fn record(&self, path: &Path, name: &str, version: &str, origin: Origin) -> Result<()> {
        let path = canonical(path)?;
        self.db.execute(
            "INSERT INTO workspaces(path, name, version, state, origin, last_seen)
                  VALUES(?1, ?2, ?3, 'live', ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                  name      = excluded.name,
                  version   = excluded.version,
                  state     = 'live',
                  last_seen = excluded.last_seen,
                  -- An explicit registration is a statement about intent, so a later
                  -- ambient touch refreshes the row without demoting how it got here.
                  origin    = CASE WHEN workspaces.origin = 'registered'
                                   THEN workspaces.origin ELSE excluded.origin END",
            turso::params![
                path_key(&path),
                name,
                version,
                origin.as_str(),
                crate::clock::now(),
            ],
        )?;
        Ok(())
    }

    /// Every sighting, path-ordered.
    pub fn sightings(&self) -> Result<Vec<Sighting>> {
        self.db.query_rows(
            "SELECT path, name, version, state, origin, last_seen
               FROM workspaces ORDER BY name, path",
            (),
            row_to_sighting,
        )
    }

    /// Every sighting declaring `name`.
    pub fn by_name(&self, name: &str) -> Result<Vec<Sighting>> {
        self.db.query_rows(
            "SELECT path, name, version, state, origin, last_seen
               FROM workspaces WHERE name = ?1 ORDER BY path",
            [name],
            row_to_sighting,
        )
    }

    /// Every live package as `(declared name, root)` — the scope of a rootless session
    /// (registry.v2.md §9).
    ///
    /// Paths are checked as they are read, so a checkout that has gone away is neither
    /// returned nor left claiming to be live. A name sighted at two live paths yields two
    /// entries: which one is "the" package is a resolution-time question, and answering it
    /// here would be the write-time guess sightings exist to avoid.
    ///
    /// The check runs **both ways**. Demoting alone would leave a returned checkout being
    /// served by every read while its row still said `missing` — and `catalog rm --missing`
    /// would then delete a row for a package the session was using. Two states with no
    /// clocks only works if both transitions are taken wherever the path is probed
    /// (cli.md §4.8).
    pub fn live_packages(&self) -> Result<Vec<(String, std::path::PathBuf)>> {
        let mut out = Vec::new();
        for sighting in self.sightings()? {
            if !sighting.path.join("knowledge.toml").is_file() {
                if sighting.state != State::Missing {
                    let _ = self.set_state(&sighting.path, State::Missing);
                }
                continue;
            }
            if sighting.state != State::Live {
                let _ = self.set_state(&sighting.path, State::Live);
            }
            out.push((sighting.name, sighting.path));
        }
        Ok(out)
    }

    // ---- registries (registry.v2.md §8) -----------------------------------------------

    /// Configure a registry, or update one already configured under this name.
    ///
    /// Keyed by the local name rather than by the URL, deliberately: re-pointing `central`
    /// at a new bucket should move the name, not leave two rows racing to be it.
    pub fn add_registry(&self, row: &RegistryRow) -> Result<()> {
        self.db.execute(
            "INSERT INTO registries(name, url, kind, priority, search_by_default)
                  VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET
                  url               = excluded.url,
                  kind              = excluded.kind,
                  priority          = excluded.priority,
                  search_by_default = excluded.search_by_default",
            turso::params![
                row.name.as_str(),
                row.url.as_str(),
                row.kind.as_str(),
                row.priority,
                i64::from(row.search_by_default),
            ],
        )?;
        Ok(())
    }

    /// Every configured registry, highest priority first and then by name — the order a
    /// fan-out would ask them in, so listing and querying agree.
    pub fn registries(&self) -> Result<Vec<RegistryRow>> {
        self.db.query_rows(
            "SELECT name, url, kind, priority, search_by_default
               FROM registries ORDER BY priority DESC, name",
            (),
            |row| {
                Ok(RegistryRow {
                    name: col_text(row, 0)?,
                    url: col_text(row, 1)?,
                    kind: col_text(row, 2)?,
                    priority: col_i64(row, 3)?,
                    search_by_default: col_i64(row, 4)? != 0,
                })
            },
        )
    }

    /// Forget a registry. Returns whether a row went.
    ///
    /// Nothing cascades: releases already pulled from it stay in the store, because they
    /// are on this disk and still readable. Removing a registry says "stop asking here",
    /// never "unlearn what it told me".
    pub fn forget_registry(&self, name: &str) -> Result<bool> {
        Ok(self
            .db
            .execute("DELETE FROM registries WHERE name = ?1", [name])?
            > 0)
    }

    // ---- store entries (registry.v2.md §5) ---------------------------------------------

    /// Record a materialized store entry.
    ///
    /// An index over `source.toml`, never the fact: the store entries are self-describing,
    /// so losing these rows costs a walk of `~/.vaire/store` and nothing else. That is why
    /// nothing here is checked against the filesystem on write — the write follows a
    /// materialization that just succeeded.
    pub fn record_release(&self, entry: &StoreEntry) -> Result<()> {
        self.db.execute(
            // A standing request is **package-scoped**, so a new version of a package that
            // has one inherits it. Without the `EXISTS`, a rootless pull of 1.0.0 followed
            // by a manifest-driven pull of 1.1.0 would leave the newcomer unrequested;
            // retention would then take 1.0.0 and the next sweep would take 1.1.0, quietly
            // undoing a request nobody withdrew. `vaire clean <name>` is the only withdrawal.
            "INSERT INTO releases(name, version, origin, registry, pinned, requested, last_used)
                  VALUES(?1, ?2, 'store', ?3, ?4,
                         MAX(?5, EXISTS(SELECT 1 FROM releases
                                         WHERE name = ?1 AND requested = 1)),
                         ?6)
             ON CONFLICT(name, version) DO UPDATE SET
                  origin    = 'store',
                  registry  = excluded.registry,
                  last_used = excluded.last_used,
                  -- A pin is a statement by the user; re-pulling the same version is not a
                  -- reason to forget it.
                  pinned    = CASE WHEN releases.pinned = 1 THEN 1 ELSE excluded.pinned END,
                  -- Likewise a standing request: a later manifest-driven pull of the same
                  -- version does not turn a package somebody asked for into a leaf the next
                  -- sweep may take.
                  requested = CASE WHEN releases.requested = 1 THEN 1 ELSE excluded.requested END",
            turso::params![
                entry.name.as_str(),
                entry.version.to_string(),
                entry.registry.clone(),
                i64::from(entry.pinned),
                i64::from(entry.requested),
                crate::clock::now(),
            ],
        )?;
        Ok(())
    }

    /// Bring a catalog forward from `found` to [`SCHEMA_VERSION`], returning whether it was
    /// brought all the way. `false` leaves the caller to recreate.
    ///
    /// This file otherwise has no migration machinery on purpose: every row is an
    /// observation, so recreating costs a rescan and nothing else. **`vaire clean` changed
    /// what that costs.** A sweep decides what to delete from what the catalog knows, so a
    /// recreated catalog is not merely empty — it is a machine that has forgotten every
    /// workspace whose lockfile was holding a release, and the next sweep would take those
    /// releases. Losing an observation used to mean re-observing it; now it can mean losing
    /// bytes. One `ADD COLUMN` is a small price for that not being true.
    /// Each step is **idempotent**, because there is no transaction spanning the schema
    /// change and the version stamp: a process killed between them leaves a catalog whose
    /// tables are already migrated and whose recorded version is not, and the next open has
    /// to be able to finish the job rather than fail on it.
    fn migrate(&self, found: u32) -> Result<bool> {
        let mut version = found;
        // v1 → v2: `releases.requested` (registry.v2.md amendment 61). Defaulted, so every
        // existing row reads as "not asked for by name", which is what those pulls were.
        if version == 1 {
            if !self.has_column("releases", "requested")? {
                self.db.execute(
                    "ALTER TABLE releases ADD COLUMN requested INTEGER NOT NULL DEFAULT 0",
                    (),
                )?;
            }
            version = 2;
        }
        if version != SCHEMA_VERSION {
            return Ok(false);
        }
        self.set_schema_version(SCHEMA_VERSION)?;
        Ok(true)
    }

    /// Whether `table` already has `column`. Asked by selecting it: a failed query is the
    /// answer, and it needs no agreement with the engine about which `PRAGMA`s it supports.
    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        Ok(self
            .db
            .query_rows(&format!("SELECT {column} FROM {table} LIMIT 0"), (), |_| {
                Ok(())
            })
            .is_ok())
    }

    /// Hold (or release) one stored version against retention and `vaire clean`.
    ///
    /// Separate from [`Catalog::record_release`], which can only ever *set* the flag: a pin
    /// that a routine pull could clear would not be a pin, so clearing one has to be its own
    /// deliberate statement. `false` for a row that is not there.
    pub fn set_pinned(
        &self,
        name: &str,
        version: crate::model::Version,
        pinned: bool,
    ) -> Result<bool> {
        Ok(self.db.execute(
            "UPDATE releases SET pinned = ?3 WHERE name = ?1 AND version = ?2",
            turso::params![name, version.to_string(), i64::from(pinned)],
        )? > 0)
    }

    /// Record (or withdraw) a standing request for a package, across every version of it.
    ///
    /// By name rather than by version, because that is how it was asked for: `vaire pull
    /// acme-core` means "have this package here", and retention replacing 1.4.1 with 1.4.2
    /// must not quietly retract the request along with the bytes.
    pub fn set_requested(&self, name: &str, requested: bool) -> Result<u64> {
        self.db.execute(
            "UPDATE releases SET requested = ?2 WHERE name = ?1",
            turso::params![name, i64::from(requested)],
        )
    }

    /// Every recorded store entry, by name then version.
    ///
    /// A row whose version text does not parse is **dropped, never defaulted**. Reporting it
    /// as `0.0.0` would be a fabricated fact with teeth: retention collects the versions of
    /// pinned rows and protects exactly those, so a pinned row reading as `0.0.0` would stop
    /// protecting the version it names and the next pull would delete it. The store's own
    /// `source.toml` is the fact here, so losing an index row costs a walk.
    pub fn releases(&self) -> Result<Vec<StoreEntry>> {
        let rows = self.db.query_rows(
            "SELECT name, version, registry, pinned, requested, last_used
               FROM releases WHERE origin = 'store' ORDER BY name, version",
            (),
            |row| {
                Ok((
                    col_text(row, 0)?,
                    col_text(row, 1)?,
                    col_opt_text(row, 2)?,
                    col_i64(row, 3)? != 0,
                    col_i64(row, 4)? != 0,
                    col_opt_i64(row, 5)?,
                ))
            },
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|(name, version, registry, pinned, requested, last_used)| {
                Some(StoreEntry {
                    name,
                    version: version.parse().ok()?,
                    registry,
                    pinned,
                    requested,
                    last_used,
                })
            })
            .collect())
    }

    /// Forget one store entry. Called when the store copy goes, so the index does not
    /// outlive what it indexes.
    pub fn forget_release(&self, name: &str, version: crate::model::Version) -> Result<bool> {
        Ok(self.db.execute(
            "DELETE FROM releases WHERE name = ?1 AND version = ?2",
            turso::params![name, version.to_string()],
        )? > 0)
    }

    /// Mark a path `missing` (it did not answer) or `live` (it did).
    pub fn set_state(&self, path: &Path, state: State) -> Result<()> {
        let key = path_key(path);
        match state {
            State::Live => self.db.execute(
                "UPDATE workspaces SET state = 'live', last_seen = ?2 WHERE path = ?1",
                turso::params![key.as_str(), crate::clock::now()],
            )?,
            State::Missing => self.db.execute(
                "UPDATE workspaces SET state = 'missing' WHERE path = ?1",
                [key.as_str()],
            )?,
        };
        Ok(())
    }

    /// Forget one sighting by path. Returns whether a row was removed.
    pub fn forget(&self, path: &Path) -> Result<bool> {
        // Rows are keyed by canonicalized path, and the path being removed is very often
        // gone — which is the whole reason someone is removing it. A literal fallback is
        // not enough: on a system where `/var` is a symlink to `/private/var`, the stored
        // key went through that link and the literal one did not, so they never match.
        let key = path_key(&canonical_best_effort(path));
        Ok(self
            .db
            .execute("DELETE FROM workspaces WHERE path = ?1", [key.as_str()])?
            > 0)
    }

    /// Forget every sighting declaring `name`. Returns how many rows went.
    pub fn forget_name(&self, name: &str) -> Result<u64> {
        self.db
            .execute("DELETE FROM workspaces WHERE name = ?1", [name])
    }

    /// Forget every sighting whose path no longer answers. The explicit bulk cleanup —
    /// nothing else ever deletes a row, so a spring clean is a command, not a timer.
    pub fn forget_missing(&self) -> Result<u64> {
        self.db
            .execute("DELETE FROM workspaces WHERE state = 'missing'", ())
    }

    /// Re-check every sighting and update its state. Cheap: one stat per row, for the
    /// package's `knowledge.toml` — a directory that survived but lost its manifest is no
    /// longer a package, so it is `missing` too. The manifest is not *read* here; whether
    /// it still declares the same name and version is a different question, asked only
    /// when a row is actually selected for resolution.
    pub fn refresh(&self) -> Result<(usize, usize)> {
        let mut live = 0;
        let mut missing = 0;
        for sighting in self.sightings()? {
            let exists = sighting.path.join("knowledge.toml").is_file();
            let state = if exists { State::Live } else { State::Missing };
            if state != sighting.state {
                self.set_state(&sighting.path, state)?;
            } else if exists {
                live += 1;
                continue;
            }
            if exists {
                live += 1;
            } else {
                missing += 1;
            }
        }
        Ok((live, missing))
    }
}

fn row_to_sighting(row: &turso::Row) -> Result<Sighting> {
    Ok(Sighting {
        path: PathBuf::from(col_text(row, 0)?),
        name: col_text(row, 1)?,
        version: col_text(row, 2)?,
        state: State::parse(&col_text(row, 3)?),
        origin: Origin::parse(&col_text(row, 4)?),
        last_seen: col_opt_i64(row, 5)?,
    })
}

/// How long to wait for another process to release the catalog. Generously above any real
/// hold time — every operation here is an upsert or a small query — so reaching it means
/// something is genuinely wrong rather than merely busy.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether an open failure means "someone else has it" rather than "it is broken".
///
/// Matched on the message because that is what the engine gives us: Turso reports the
/// exclusive-open conflict as a generic error whose text names the locked file. The
/// consequence of guessing wrong is asymmetric — a missed lock error deletes a healthy
/// catalog — so this errs toward treating anything lock-shaped as contention, and lets the
/// [`LOCK_WAIT`] timeout be the thing that eventually gives up.
fn is_locked(error: &VaireError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("lock")
}

/// How long to wait before the next open attempt. Ramps to keep a crowd from re-colliding
/// in lockstep, and is offset by the process id so two processes that started together do
/// not stay in step with each other.
fn backoff(attempt: u32) -> std::time::Duration {
    let base = 2u64.saturating_pow(attempt.min(6)); // 2…64ms
    let jitter = u64::from(std::process::id() % 7);
    std::time::Duration::from_millis(base + jitter)
}

/// The stored form of a path. Forward slashes are *not* imposed: this is machine-local
/// state naming machine-local directories, and a Windows path is only ever compared with
/// another Windows path.
fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Canonicalize as much of `path` as still exists, keeping the rest verbatim.
///
/// For a path that resolves this is plain canonicalization. For one that does not — a
/// deleted checkout — it resolves the nearest surviving ancestor and re-appends the
/// missing tail, so the result matches the key a `record` wrote back when the directory
/// was there.
fn canonical_best_effort(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    let mut tail = Vec::new();
    let mut cursor = path;
    while let Some(parent) = cursor.parent() {
        match cursor.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if let Ok(base) = std::fs::canonicalize(parent) {
            let mut resolved = base;
            resolved.extend(tail.iter().rev());
            return resolved;
        }
        cursor = parent;
    }
    path.to_path_buf()
}

fn canonical(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))
}

/// Delete a catalog file and its WAL sidecars — Turso does not checkpoint on close, so
/// the sidecars carry committed rows and a database file alone is not the database.
fn remove_db_files(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        // Built from the OS string, not from `Display`: a path that is not valid UTF-8
        // would come back lossily converted, and the `-wal` name we constructed would then
        // name a file that does not exist. The removal would report success while the real
        // sidecar survived — and a recreated database would pick the old committed rows
        // straight back up, which is precisely what this function exists to prevent.
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
