//! The Turso facade — one async→sync bridge, shared by every database Vairë keeps.
//!
//! Turso's embedded API is async (it owns its own io_uring I/O). Rather than colour the
//! whole CLI async, a [`Db`] owns one current-thread `tokio` runtime and `block_on`s every
//! call behind a synchronous surface. Callers stay sync; async stops here.
//!
//! Two databases use this: the per-package index ([`crate::index::Index`], one per package,
//! effectively single-writer) and the machine-level catalog ([`crate::catalog::Catalog`],
//! shared across processes). They differ entirely in schema and lifecycle, and not at all
//! in how they talk to Turso — so the bridge, and the two traps below, live in one place.
//!
//! * **Row-returning statements must go through [`Db::query_rows`]/[`Db::query_opt`]**,
//!   never [`Db::execute`] — including `PRAGMA`s. Turso answers a row arriving during
//!   `execute` with `Misuse("unexpected row during execution")`.
//! * **Turso does not checkpoint the WAL on close**, so a database file is not
//!   self-contained: anything moving one must move its `-wal`/`-shm` sidecars too, or
//!   explicitly checkpoint first.

use std::future::Future;
use std::path::Path;

use turso::{Builder, Connection, Database, IntoParams, Row, Value};

use crate::error::{Result, VaireError};

/// An open database: a Turso connection plus the runtime that drives it.
pub struct Db {
    rt: tokio::runtime::Runtime,
    // Held so the database outlives the connection; the connection does the work.
    _db: Database,
    conn: Connection,
}

impl Db {
    /// Build the runtime, open the local Turso file, and connect. The experimental index
    /// method is enabled per-connection so the native FTS index is available to schemas
    /// that use one.
    pub fn connect(path: &Path) -> Result<Db> {
        // A bare current-thread runtime: Turso owns its own async I/O, so we need none of
        // tokio's resource drivers (no `enable_all`, no io/time features) — the runtime
        // exists only to `block_on` Turso's futures.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| VaireError::IndexCorrupt(format!("tokio runtime: {e}")))?;
        let path_str = path.to_string_lossy().into_owned();
        let db = rt.block_on(async {
            Builder::new_local(&path_str)
                .experimental_index_method(true)
                .build()
                .await
        })?;
        let conn = db.connect()?;
        Ok(Db { rt, _db: db, conn })
    }

    /// Block on a future using this database's own runtime — the sole async→sync bridge.
    fn block<F: Future>(&self, fut: F) -> F::Output {
        self.rt.block_on(fut)
    }

    /// Run a statement, returning the number of rows changed. Use for INSERT/UPDATE/DELETE
    /// and DDL; a statement that yields rows must go through [`Db::query_rows`].
    pub fn execute(&self, sql: &str, params: impl IntoParams) -> Result<u64> {
        Ok(self.block(self.conn.execute(sql, params))?)
    }

    /// Run a query and map every row, collecting into a `Vec`.
    pub fn query_rows<T, F>(&self, sql: &str, params: impl IntoParams, mut map: F) -> Result<Vec<T>>
    where
        F: FnMut(&Row) -> Result<T>,
    {
        self.block(async move {
            let mut rows = self.conn.query(sql, params).await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                out.push(map(&row)?);
            }
            Ok(out)
        })
    }

    /// Run a query and map its **first** row, or `None` if it returned none.
    pub fn query_opt<T, F>(&self, sql: &str, params: impl IntoParams, map: F) -> Result<Option<T>>
    where
        F: FnOnce(&Row) -> Result<T>,
    {
        self.block(async move {
            let mut rows = self.conn.query(sql, params).await?;
            match rows.next().await? {
                Some(row) => Ok(Some(map(&row)?)),
                None => Ok(None),
            }
        })
    }

    /// A single `i64` scalar (e.g. `SELECT count(*)`), defaulting to `0` for no rows.
    pub fn scalar_i64(&self, sql: &str, params: impl IntoParams) -> Result<i64> {
        Ok(self.query_opt(sql, params, |r| col_i64(r, 0))?.unwrap_or(0))
    }

    /// Run `f` inside a transaction: `BEGIN`, then `COMMIT` on success or `ROLLBACK` on
    /// error. All methods take `&self`, so the closure freely calls back in — there is one
    /// connection, and the transaction is connection-wide.
    pub fn with_tx<T>(&self, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
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
}

// --- Row extractors -----------------------------------------------------------------------
// Turso hands back a `Value` enum; these pull typed columns with a clear error on a type
// mismatch, so the query sites read much like rusqlite's `row.get::<_, T>(i)`.

/// A required `TEXT` column.
pub fn col_text(row: &Row, i: usize) -> Result<String> {
    match row.get_value(i)? {
        Value::Text(s) => Ok(s),
        other => Err(type_err(i, "TEXT", &other)),
    }
}

/// A nullable `TEXT` column: SQL `NULL` → `None`.
pub fn col_opt_text(row: &Row, i: usize) -> Result<Option<String>> {
    match row.get_value(i)? {
        Value::Text(s) => Ok(Some(s)),
        Value::Null => Ok(None),
        other => Err(type_err(i, "TEXT or NULL", &other)),
    }
}

/// A required `INTEGER` column.
pub fn col_i64(row: &Row, i: usize) -> Result<i64> {
    match row.get_value(i)? {
        Value::Integer(n) => Ok(n),
        other => Err(type_err(i, "INTEGER", &other)),
    }
}

/// A nullable `INTEGER` column: SQL `NULL` → `None`.
pub fn col_opt_i64(row: &Row, i: usize) -> Result<Option<i64>> {
    match row.get_value(i)? {
        Value::Integer(n) => Ok(Some(n)),
        Value::Null => Ok(None),
        other => Err(type_err(i, "INTEGER or NULL", &other)),
    }
}

/// A required `INTEGER` column narrowed to `u32` (line numbers, counts).
///
/// A value that does not fit is an error rather than a wrap: silently turning a negative
/// or oversized row into a plausible line number would hide the corruption instead of
/// reporting it.
pub fn col_u32(row: &Row, i: usize) -> Result<u32> {
    let value = col_i64(row, i)?;
    u32::try_from(value).map_err(|_| {
        VaireError::IndexCorrupt(format!("column {i}: {value} is out of range for a u32"))
    })
}

/// A required `REAL` column (e.g. `vector_distance_cos`, `fts_score`); an `INTEGER` widens.
pub fn col_f64(row: &Row, i: usize) -> Result<f64> {
    match row.get_value(i)? {
        Value::Real(f) => Ok(f),
        Value::Integer(n) => Ok(n as f64),
        other => Err(type_err(i, "REAL", &other)),
    }
}

/// A required `BLOB` column.
pub fn col_blob(row: &Row, i: usize) -> Result<Vec<u8>> {
    match row.get_value(i)? {
        Value::Blob(b) => Ok(b),
        other => Err(type_err(i, "BLOB", &other)),
    }
}

fn type_err(i: usize, want: &str, got: &Value) -> VaireError {
    VaireError::IndexCorrupt(format!("column {i}: expected {want}, got {got:?}"))
}
