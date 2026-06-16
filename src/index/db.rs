//! Index storage: SQLite connection, schema, and open/migrate (design.md §9).
//!
//! Storage shape (design.md §9):
//! - `nodes(id, type, path, frontmatter_json, superseded_by)` — the node table.
//! - `edges(from_id, to_id, ref_type, source_file, line)` — the graph.
//! - `unresolved(record_id, type_guess, descriptor, source_file, line)` — loose ends.
//! - `sections_fts` — an FTS5 virtual table over prose (file is the returned unit).
//! - `embeddings(section_id, content_hash, vector BLOB)` — per-section vectors +
//!   the content-hash cache, brute-force cosine in Rust (no separate vector store).
//! - `schema_version(version)` — the stable version anchor (see [`SCHEMA_VERSION`]).
//! - `meta(key, value)` — `last_indexed_commit`, `index_source`, …
//!
//! WAL mode (`index.db-wal` / `index.db-shm`) so reads don't block the post-commit
//! reindex. The DB is gitignored and per-checkout: each machine rebuilds its own.

use std::path::Path;

use rusqlite::Connection;

use crate::error::{Result, VaireError};

/// The current index schema version, stored in the `schema_version` table. **Bump this on
/// any change to the schema** (a new column/table, changed semantics): `vaire index` then
/// rebuilds from scratch, and read commands refuse a mismatched index (directing to
/// `vaire index --full`) instead of misbehaving on an unexpected shape.
pub const SCHEMA_VERSION: u32 = 1;

/// SQL that creates the full schema on a fresh database. Kept inline (rather than a
/// separate `.sql` asset) so the binary is self-contained.
pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- The stable anchor: a one-row table whose shape NEVER changes across versions, so any
-- future build can read it to learn how to migrate everything else. Bump SCHEMA_VERSION
-- (below) on any schema change; `vaire index` then rebuilds, and reads refuse a mismatch.
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE IF NOT EXISTS nodes (
    id            TEXT PRIMARY KEY,
    type          TEXT NOT NULL,
    path          TEXT NOT NULL,
    frontmatter   TEXT NOT NULL,        -- JSON
    superseded_by TEXT                  -- nullable redirect target
);

-- Every parsed (id, path) pair, WITHOUT a unique constraint, so duplicate composed IDs
-- survive indexing for `vaire check` to report (the duplicate-entity guard, design.md §9).
-- `nodes` keeps only the first occurrence (INSERT OR IGNORE); this keeps them all.
CREATE TABLE IF NOT EXISTS node_files (
    id   TEXT NOT NULL,
    path TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS node_files_id ON node_files(id);

CREATE TABLE IF NOT EXISTS edges (
    from_id     TEXT NOT NULL,
    to_id       TEXT NOT NULL,
    ref_type    TEXT NOT NULL,          -- frontmatter key, or 'inline'
    source_file TEXT NOT NULL,
    line        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS edges_to   ON edges(to_id);
CREATE INDEX IF NOT EXISTS edges_from ON edges(from_id);

CREATE TABLE IF NOT EXISTS unresolved (
    record_id   TEXT NOT NULL,
    type_guess  TEXT,                   -- nullable: [[?: ...]] has no type
    descriptor  TEXT NOT NULL,
    source_file TEXT NOT NULL,
    line        INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS sections_fts USING fts5(
    node_id UNINDEXED,
    heading,
    line UNINDEXED,
    body
);

CREATE TABLE IF NOT EXISTS embeddings (
    node_id      TEXT NOT NULL,
    section_line INTEGER NOT NULL,
    content_hash BLOB NOT NULL,
    vector       BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS embeddings_hash ON embeddings(content_hash);

-- Content-hash embedding cache (design.md §9): vectors keyed by section-text hash,
-- decoupled from any node/path so an unchanged section reuses its vector across
-- incremental reindexes. Survives delete_file; only `--full` (which recreates the db)
-- clears it. Without this, "rebuildable in seconds" breaks once embeddings exist.
CREATE TABLE IF NOT EXISTS embed_cache (
    content_hash BLOB PRIMARY KEY,
    vector       BLOB NOT NULL
);
"#;

/// An open handle to the derived index.
pub struct Index {
    conn: Connection,
}

impl Index {
    /// Open an existing index. Errors map to the documented exit codes: a missing file
    /// is [`VaireError::IndexNotBuilt`] (exit `4`); a structurally broken file is
    /// [`VaireError::IndexCorrupt`] (exit `3`).
    pub fn open(path: &Path) -> Result<Index> {
        if !path.exists() {
            return Err(VaireError::IndexNotBuilt(path.display().to_string()));
        }
        let conn = Connection::open(path).map_err(|e| VaireError::IndexCorrupt(e.to_string()))?;
        Ok(Index { conn })
    }

    /// Create (or recreate) the index file and install the schema.
    pub fn create(path: &Path) -> Result<Index> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        // Stamp the schema version (fresh db ⇒ exactly one row).
        conn.execute("DELETE FROM schema_version", [])?;
        conn.execute(
            "INSERT INTO schema_version(version) VALUES(?1)",
            [SCHEMA_VERSION as i64],
        )?;
        Ok(Index { conn })
    }

    /// The schema version stamped in the index, or `None` if absent/unreadable (an index
    /// built before versioning, or a corrupt one) — treated as a mismatch by callers.
    pub fn schema_version(&self) -> Option<u32> {
        self.conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get::<_, i64>(0)
            })
            .ok()
            .map(|v| v as u32)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Read a `meta` value, e.g. `last_indexed_commit`.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }
}
