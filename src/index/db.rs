//! Index storage: the Turso connection, schema, and open/create (design.md §9).
//!
//! The engine is **Turso Database** — the Rust rewrite of SQLite (crate `turso`) — used as
//! a local, in-process, embedded file engine (never a server, never the network) for its
//! native full-text search and native vectors (issue #2 M3).
//!
//! Storage shape (design.md §9):
//! - `nodes(id, type, path, frontmatter_json, superseded_by, package)` — the node table;
//!   `package` is the owning package (manifest `name`, issue #2 M4).
//! - `edges(from_id, to_id, to_package, ref_type, source_file, line)` — the graph; `to_id`
//!   is the within-package address and `to_package` (nullable) carries an `@pkg/` target.
//! - `unresolved(record_id, type_guess, descriptor, source_file, line)` — loose ends.
//! - `sections(node_id, heading, line, body)` — prose, with a native **FTS index**
//!   (`sections_fts`, weighted heading>body) over `(heading, body)`; the file is the unit.
//! - `embeddings(node_id, section_line, content_hash, vector)` — per-section vectors as
//!   Float32-dense blobs (little-endian `f32`, which is exactly Turso's own vector layout),
//!   queried with the native `vector_distance_cos`.
//! - `schema_version(version)` — the stable version anchor (see [`SCHEMA_VERSION`]).
//! - `meta(key, value)` — `last_indexed_commit`, `index_source`, …
//!
//! **Async containment.** Turso's embedded API is async (io_uring). Rather than colour the
//! whole CLI async, the [`Index`] owns one current-thread `tokio` runtime and `block_on`s
//! every `turso` call behind a *synchronous* facade ([`Index::execute`],
//! [`Index::query_rows`], …). Callers stay sync; async stops at this boundary.
//!
//! WAL is Turso's default journal mode. The DB is gitignored and per-checkout: each machine
//! rebuilds its own from the Markdown.

use std::path::Path;

use turso::{IntoParams, Row};

use crate::db::Db;
pub use crate::db::{col_blob, col_f64, col_i64, col_opt_i64, col_opt_text, col_text, col_u32};
use crate::error::{Result, VaireError};

/// The current index schema version, stored in the `schema_version` table. **Bump this on
/// any change to the schema** (a new column/table, changed semantics): `vaire index` then
/// rebuilds from scratch, and read commands refuse a mismatched index (directing to
/// `vaire index --full`) instead of misbehaving on an unexpected shape.
///
/// v2: migrated `rusqlite`/FTS5 → Turso; the `sections_fts` FTS5 *virtual table* became a
/// regular `sections` table with a native FTS *index* (issue #2 M3).
/// v3: package-aware index (issue #2 M4) — `nodes.package` (the owning package, from the
/// manifest `name`) and `edges.to_package` (NULL = local; set for an `@pkg/` target).
/// v4: lookup indexes for incremental replacement and section/embedding joins.
/// v5: `nodes.alias_text` — name + aliases denormalized for alias matching.
/// v6: `diagram_links(from_id, path)` — every diagram file a node's prose links to,
/// recorded regardless of whether that file currently carries a `vaire/` marker (or
/// even exists), so an edit to an external diagram file can invalidate the nodes that
/// link to it (issue #23) without relying on there already being a `diagram` edge.
pub const SCHEMA_VERSION: u32 = 6;

/// The schema as individual statements, run in order on a fresh database. Kept inline
/// (rather than a `.sql` asset) so the binary is self-contained. No `PRAGMA`s: WAL is
/// Turso's default and there are no foreign-key constraints to enable.
const SCHEMA_STMTS: &[&str] = &[
    // The stable anchor: a one-row table whose shape NEVER changes across versions, so any
    // future build can read it to learn how to migrate everything else.
    "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)",
    "CREATE TABLE IF NOT EXISTS nodes (
        id            TEXT PRIMARY KEY,
        type          TEXT NOT NULL,
        path          TEXT NOT NULL,
        frontmatter   TEXT NOT NULL,        -- JSON
        superseded_by TEXT,                 -- nullable redirect target
        package       TEXT NOT NULL,        -- the owning package (manifest `name`)
        -- The effective display name plus every alias, lowercased and NUL-joined. Purely
        -- derived from `frontmatter`, denormalized so alias matching does not have to
        -- JSON-parse every node's whole frontmatter on every query (design.md §8).
        alias_text    TEXT NOT NULL DEFAULT ''
    )",
    "CREATE INDEX IF NOT EXISTS nodes_path ON nodes(path)",
    // Every parsed (id, path) pair, WITHOUT a unique constraint, so duplicate composed IDs
    // survive indexing for `vaire check` to report (the duplicate-entity guard). `nodes`
    // keeps only the first occurrence (INSERT OR IGNORE); this keeps them all.
    "CREATE TABLE IF NOT EXISTS node_files (id TEXT NOT NULL, path TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS node_files_id ON node_files(id)",
    "CREATE INDEX IF NOT EXISTS node_files_path ON node_files(path)",
    "CREATE TABLE IF NOT EXISTS edges (
        from_id     TEXT NOT NULL,
        to_id       TEXT NOT NULL,          -- the within-package address (no @pkg/ prefix)
        to_package  TEXT,                   -- nullable: set for an @pkg/ cross-package target
        ref_type    TEXT NOT NULL,          -- frontmatter key, or 'inline'
        source_file TEXT NOT NULL,
        line        INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS edges_to   ON edges(to_id)",
    "CREATE INDEX IF NOT EXISTS edges_from ON edges(from_id)",
    "CREATE INDEX IF NOT EXISTS edges_source_file ON edges(source_file)",
    // Every diagram file a node's prose links to (issue #23), independent of whether that
    // file currently carries a `vaire/` marker or even exists — this is the *link*
    // relationship, not the derived edge, so incremental indexing can invalidate a node
    // when its linked diagram file changes even before that file had any marker at all.
    "CREATE TABLE IF NOT EXISTS diagram_links (
        from_id TEXT NOT NULL,
        path    TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS diagram_links_from ON diagram_links(from_id)",
    "CREATE INDEX IF NOT EXISTS diagram_links_path ON diagram_links(path)",
    "CREATE TABLE IF NOT EXISTS unresolved (
        record_id   TEXT NOT NULL,
        type_guess  TEXT,                   -- nullable: [[?: ...]] has no type
        descriptor  TEXT NOT NULL,
        source_file TEXT NOT NULL,
        line        INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS unresolved_source_file ON unresolved(source_file)",
    // Prose sections: a regular table now (was an FTS5 virtual table). The full-text search
    // lives in a native FTS *index* over (heading, body), with heading weighted above body
    // for BM25 ranking (`fts_score`).
    "CREATE TABLE IF NOT EXISTS sections (
        node_id TEXT NOT NULL,
        heading TEXT NOT NULL,
        line    INTEGER NOT NULL,
        body    TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS sections_node_line ON sections(node_id, line)",
    // Per-section vectors. `vector` is a little-endian f32 blob — identical to Turso's own
    // Float32-dense layout — so `vector_distance_cos` reads it natively (no conversion).
    "CREATE TABLE IF NOT EXISTS embeddings (
        node_id      TEXT NOT NULL,
        section_line INTEGER NOT NULL,
        content_hash BLOB NOT NULL,
        vector       BLOB NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS embeddings_hash ON embeddings(content_hash)",
    "CREATE INDEX IF NOT EXISTS embeddings_node_line ON embeddings(node_id, section_line)",
    // Content-hash embedding cache (design.md §9): vectors keyed by section-text hash,
    // decoupled from any node/path so an unchanged section reuses its vector across
    // reindexes. Survives delete_file, and a full rebuild carries it over from the index
    // it replaces (same embedder only) — otherwise every rebuild of a non-Git corpus, and
    // every `--working-tree` run, would re-embed the whole corpus. `--re-embed` is the way
    // to force fresh vectors. Without this, "rebuildable in seconds" breaks once
    // embeddings exist.
    "CREATE TABLE IF NOT EXISTS embed_cache (
        content_hash BLOB PRIMARY KEY,
        vector       BLOB NOT NULL
    )",
];

/// Kept separate from [`SCHEMA_STMTS`] so a full rebuild can load every section before
/// Turso/Tantivy builds its derived search structure. Maintaining the FTS index for every
/// individual insert is dramatically more expensive on a cold corpus.
const FTS_INDEX_STMT: &str = "CREATE INDEX IF NOT EXISTS sections_fts ON sections USING fts (heading, body) \
     WITH (weights='heading=2.0,body=1.0')";

/// An open handle to the derived index — the shared Turso facade plus this schema's
/// discipline (version gate, `meta`, the deferred FTS index).
pub struct Index {
    db: Db,
}

impl Index {
    /// Open an existing index. Errors map to the documented exit codes: a missing file
    /// is [`VaireError::IndexNotBuilt`] (exit `4`); a file Turso cannot open is
    /// [`VaireError::IndexCorrupt`] (exit `3`).
    pub fn open(path: &Path) -> Result<Index> {
        if !path.exists() {
            return Err(VaireError::IndexNotBuilt(path.display().to_string()));
        }
        Self::connect(path).map_err(|e| VaireError::IndexCorrupt(e.to_string()))
    }

    /// Create (or recreate) the index file and install the schema.
    pub fn create(path: &Path) -> Result<Index> {
        let index = Self::create_for_bulk_load(path)?;
        index.ensure_fts_index()?;
        Ok(index)
    }

    /// Create the relational schema but defer its derived FTS index. Only the index builder
    /// uses this variant, immediately creating the FTS index after its bulk load succeeds.
    pub(crate) fn create_for_bulk_load(path: &Path) -> Result<Index> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let index = Self::connect(path)?;
        for stmt in SCHEMA_STMTS {
            index.execute(stmt, ())?;
        }
        // Stamp the schema version (fresh db ⇒ exactly one row).
        index.execute("DELETE FROM schema_version", ())?;
        index.execute(
            "INSERT INTO schema_version(version) VALUES(?1)",
            [i64::from(SCHEMA_VERSION)],
        )?;
        Ok(index)
    }

    /// Ensure the native full-text index exists after a bulk insert.
    pub(crate) fn ensure_fts_index(&self) -> Result<()> {
        self.execute(FTS_INDEX_STMT, ())?;
        Ok(())
    }

    fn connect(path: &Path) -> Result<Index> {
        Ok(Index {
            db: Db::connect(path)?,
        })
    }

    /// Run a statement, returning the number of rows changed. Use for INSERT/UPDATE/DELETE
    /// and DDL; a statement that yields rows must go through [`Index::query_rows`].
    pub fn execute(&self, sql: &str, params: impl IntoParams) -> Result<u64> {
        self.db.execute(sql, params)
    }

    /// Run a query and map every row, collecting into a `Vec`.
    pub fn query_rows<T, F>(&self, sql: &str, params: impl IntoParams, map: F) -> Result<Vec<T>>
    where
        F: FnMut(&Row) -> Result<T>,
    {
        self.db.query_rows(sql, params, map)
    }

    /// Run a query and map its **first** row, or `None` if it returned none.
    pub fn query_opt<T, F>(&self, sql: &str, params: impl IntoParams, map: F) -> Result<Option<T>>
    where
        F: FnOnce(&Row) -> Result<T>,
    {
        self.db.query_opt(sql, params, map)
    }

    /// A single `i64` scalar (e.g. `SELECT count(*)`), defaulting to `0` for no rows.
    pub fn scalar_i64(&self, sql: &str, params: impl IntoParams) -> Result<i64> {
        self.db.scalar_i64(sql, params)
    }

    /// Run `f` inside a transaction: `BEGIN`, then `COMMIT` on success or `ROLLBACK` on
    /// error. All facade methods take `&self`, so the closure freely calls back into the
    /// index (there is one connection; the transaction is connection-wide).
    pub fn with_tx<T>(&self, f: impl FnOnce(&Index) -> Result<T>) -> Result<T> {
        self.execute("BEGIN", ())?;
        match f(self) {
            Ok(v) => {
                self.execute("COMMIT", ())?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.execute("ROLLBACK", ());
                Err(e)
            }
        }
    }

    /// The schema version stamped in the index, or `None` if absent/unreadable (an index
    /// built before versioning, or a corrupt one) — treated as a mismatch by callers.
    pub fn schema_version(&self) -> Option<u32> {
        self.query_opt("SELECT version FROM schema_version LIMIT 1", (), |r| {
            col_i64(r, 0)
        })
        .ok()
        .flatten()
        .map(|v| v as u32)
    }

    /// Overwrite the stamped schema version. Primarily a test seam for simulating an index
    /// written by a different `vaire` version.
    pub fn set_schema_version(&self, version: u32) -> Result<()> {
        self.execute("DELETE FROM schema_version", ())?;
        self.execute(
            "INSERT INTO schema_version(version) VALUES(?1)",
            [i64::from(version)],
        )?;
        Ok(())
    }

    /// Read a `meta` value, e.g. `last_indexed_commit`.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.query_opt("SELECT value FROM meta WHERE key = ?1", [key], |r| {
            col_text(r, 0)
        })
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}
