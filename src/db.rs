//! The Turso facade — one async→sync bridge, shared by every database Vairë keeps.
//!
//! Turso's embedded API is async (it owns its own io_uring I/O). Rather than colour the
//! whole CLI async, a [`Db`] owns one current-thread `tokio` runtime and `block_on`s every
//! call behind a synchronous surface. Callers stay sync; async stops here.
//!
//! Two databases use this: the per-package index ([`crate::index::Index`], one per package)
//! and the machine-level catalog ([`crate::catalog::Catalog`]). They differ entirely in
//! schema and lifecycle, and not at all in how they talk to Turso — so the bridge, the lock
//! every open takes ([`DbLock`]), and the traps below live in one place.
//!
//! * **Row-returning statements must go through [`Db::query_rows`]/[`Db::query_opt`]**,
//!   never [`Db::execute`] — including `PRAGMA`s. Turso answers a row arriving during
//!   `execute` with `Misuse("unexpected row during execution")`.
//! * **Turso does not checkpoint the WAL on close**, so a database file is not
//!   self-contained: anything moving one must move its `-wal`/`-shm` sidecars too, or
//!   explicitly checkpoint first.
//! * **Turso takes an exclusive lock when a database is opened**, and a second process that
//!   opens it — even to read — fails on the spot. Every open therefore goes through a
//!   [`DbLock`] first, which a second process *waits* on instead.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak, mpsc};
use std::time::Duration;

use fs4::fs_std::FileExt;
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

// --- Cross-process locks ------------------------------------------------------------------

/// Bounds how long this process waits for a database another vaire process holds, in whole
/// seconds. Unset, the command line waits for as long as it takes.
pub const LOCK_TIMEOUT_ENV: &str = "VAIRE_LOCK_TIMEOUT";

/// How long a waiting process stays quiet before saying so. Most contention is a read that
/// finishes within milliseconds, and announcing each of those would be noise; a wait that
/// outlasts this is behind something real (usually `vaire index`), and a command sitting
/// silent through it looks hung.
const WAIT_NOTICE_AFTER: Duration = Duration::from_secs(1);

/// This process's wait limit in milliseconds; `u64::MAX` means "for as long as it takes".
static LOCK_WAIT_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Bound (or, with `None`, unbound) how long every lock in this process waits.
///
/// Process-wide on purpose: the right answer depends on who is waiting, not on which
/// database. A person at a terminal queues behind `vaire index` like behind any other job,
/// and can press Ctrl-C; an agent calling `vaire mcp` is mid-conversation, and is better
/// served by an error it can act on than by a call that never returns. The binary decides
/// once, at startup.
pub fn set_lock_wait(limit: Option<Duration>) {
    let ms = limit.map_or(u64::MAX, |limit| {
        u64::try_from(limit.as_millis()).unwrap_or(u64::MAX - 1)
    });
    LOCK_WAIT_MS.store(ms, Ordering::Relaxed);
}

fn lock_wait() -> Option<Duration> {
    match LOCK_WAIT_MS.load(Ordering::Relaxed) {
        u64::MAX => None,
        ms => Some(Duration::from_millis(ms)),
    }
}

/// The wait limit [`LOCK_TIMEOUT_ENV`] asks for, or `default` when it is unset.
pub fn lock_wait_from_env(default: Option<Duration>) -> Result<Option<Duration>> {
    match std::env::var(LOCK_TIMEOUT_ENV) {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(secs) => Ok(Some(Duration::from_secs(secs))),
            Err(_) => Err(VaireError::Usage(format!(
                "{LOCK_TIMEOUT_ENV} must be a whole number of seconds, not {value:?}"
            ))),
        },
        Err(e) => Err(VaireError::Usage(format!("{LOCK_TIMEOUT_ENV}: {e}"))),
    }
}

/// Why [`DbLock::acquire`] came back without the lock.
#[derive(Debug)]
pub enum LockError {
    /// Another process held it for longer than [`set_lock_wait`] allows.
    TimedOut(Duration),
    /// The lock file could not be created, opened, or locked.
    Io(std::io::Error),
}

impl From<std::io::Error> for LockError {
    fn from(e: std::io::Error) -> Self {
        LockError::Io(e)
    }
}

/// An exclusive, cross-process lock, taken **before** a database is opened and held for as
/// long as its handle lives.
///
/// Turso already refuses a second process that opens a file it has open — but it refuses on
/// the spot, which left every caller choosing between failing and polling. This puts a lock
/// file of vaire's own in front of it: an OS file lock (`flock` on Unix, `LockFileEx` on
/// Windows), so a process that finds it taken is parked by the kernel and woken when the
/// holder lets go — a queue, not a retry loop. The kernel also releases it when its holder
/// dies, so there is no such thing as a stale lock, and nothing ever deletes a lock file.
///
/// Three properties callers rely on:
///
/// * **A file beside the database, never the database itself.** A rebuild replaces
///   `index.db` by renaming a new file over it; a process waiting on the old file would
///   wake up holding a lock on a file nobody uses any more.
/// * **Re-entrant within a process.** The OS lock belongs to an open file, so a second
///   acquisition by the same process — a command holding its index while a helper opens it
///   again — would wait on itself forever. A process shares one lock per path instead,
///   released when the last holder drops it. Exclusion is between processes, which is the
///   only exclusion Turso needs.
/// * **Exclusive only.** A shared mode for readers would buy nothing: Turso lets one process
///   have the file open at a time regardless, so readers serialize anyway — for the
///   milliseconds a read takes.
///
/// No operating system promises file-lock waiters strict arrival order, and this does not
/// either; with holds this short, nobody waits long behind anything but a build.
#[derive(Clone)]
pub struct DbLock {
    _held: Arc<HeldLock>,
}

struct HeldLock {
    file: File,
    path: PathBuf,
}

impl DbLock {
    /// Take the lock at `path` (creating the file if needed), waiting within this process's
    /// limit ([`set_lock_wait`]) for any other process that holds it. `what` names the
    /// database in the notice a long wait prints.
    pub fn acquire(path: &Path, what: &str) -> std::result::Result<DbLock, LockError> {
        // Keyed by the canonical path, so two spellings of one directory — a symlinked home,
        // macOS's `/var` → `/private/var` — cannot become two locks that wait on each other.
        let path = canonical_lock_path(path)?;
        if let Some(held) = held_locks().get(&path).and_then(Weak::upgrade) {
            return Ok(DbLock { _held: held });
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let file = wait_for(file, what)?;
        let held = Arc::new(HeldLock {
            file,
            path: path.clone(),
        });
        held_locks().insert(path, Arc::downgrade(&held));
        Ok(DbLock { _held: held })
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        // Unlocked explicitly rather than left to the close: Windows releases a closed
        // handle's locks "when resources allow", and the next process in line should not
        // wait on that.
        let _ = FileExt::unlock(&self.file);
        let mut held = held_locks();
        // Only this lock's own entry — another thread may already have taken the path again.
        if held
            .get(&self.path)
            .is_some_and(|entry| entry.strong_count() == 0)
        {
            held.remove(&self.path);
        }
    }
}

/// Lock `file`, parking this process until the holder lets go.
///
/// The blocking lock call has no timeout of its own, so a contended wait runs it on a thread
/// and waits for the answer — which is what lets one wait both announce itself after a
/// second and give up at a deadline. A thread whose waiter has already given up still gets
/// the lock eventually; finding nobody to hand it to, it lets go at once.
fn wait_for(file: File, what: &str) -> std::result::Result<File, LockError> {
    if FileExt::try_lock_exclusive(&file)? {
        return Ok(file);
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = FileExt::lock_exclusive(&file).map(|()| file);
        if let Err(mpsc::SendError(Ok(file))) = tx.send(outcome) {
            let _ = FileExt::unlock(&file);
        }
    });
    let limit = lock_wait();
    let quiet = limit.map_or(WAIT_NOTICE_AFTER, |limit| limit.min(WAIT_NOTICE_AFTER));
    let answer = match rx.recv_timeout(quiet) {
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let remaining = limit.map(|limit| limit.saturating_sub(quiet));
            if remaining != Some(Duration::ZERO) {
                eprintln!("waiting for another vaire process to release {what}…");
            }
            match remaining {
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                Some(remaining) => rx.recv_timeout(remaining),
            }
        }
        answer => answer,
    };
    match answer {
        Ok(outcome) => Ok(outcome?),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(LockError::TimedOut(limit.unwrap_or_default())),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(LockError::Io(std::io::Error::other(
            "the lock wait ended without an answer",
        ))),
    }
}

/// `path` with its directory canonicalized. The file itself may not exist yet.
fn canonical_lock_path(path: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a lock file path: {}", path.display()),
        )
    })?;
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    Ok(std::fs::canonicalize(dir)?.join(name))
}

/// The locks this process holds, by canonical path.
fn held_locks() -> MutexGuard<'static, HashMap<PathBuf, Weak<HeldLock>>> {
    static HELD: OnceLock<Mutex<HashMap<PathBuf, Weak<HeldLock>>>> = OnceLock::new();
    HELD.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Whether a failed [`Db::connect`] means another process has the file open.
///
/// With a [`DbLock`] in front of every open, that is a process outside vaire's lock — an
/// older vaire, or another tool opening the file directly. Turso reports it as a plain
/// `Error` whose text is all the structure there is: `Locking error: …` on every platform
/// (what follows differs between `fcntl` and `LockFileEx`). Matched on that prefix rather
/// than on "lock" anywhere in the message, which a path such as `~/blocks/` also contains.
pub fn is_locked(error: &VaireError) -> bool {
    matches!(
        error,
        VaireError::Turso(turso::Error::Error(message)) if message.starts_with("Locking error")
    )
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
