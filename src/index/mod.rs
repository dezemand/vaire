//! The derived index (design.md §9).
//!
//! A Rust + SQLite engine that holds **no truth of its own** — the deeds live in the
//! files; this weaves them into a queryable tapestry. It is disposable, rebuildable in
//! seconds, and **never writes the corpus**. Everything it owns lives in
//! `.vaire/index.db` (WAL-mode) under the repo root.
//!
//! - [`db`]    — connection, schema, migrations: edges, FTS5, embeddings, meta.
//! - [`build`] — `vaire index`: full + incremental, commit-bound (commit-as-publish).
//! - [`query`] — graph reads backing `resolve` / `backlinks` / `refs` / `unresolved`.
//! - [`check`] — the integrity guards ID-based discovery makes possible.

pub mod build;
pub mod check;
pub mod db;
pub mod query;

pub use db::Index;
