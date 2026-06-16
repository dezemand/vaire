//! Vairë — a derived reference-graph index over a Markdown knowledge corpus.
//!
//! The corpus (a Git repo of Markdown) is the source of truth; this crate builds
//! and queries a disposable SQLite index over it. The crate **never writes the
//! corpus** — the only thing it writes is `.vaire/` (design.md §9).
//!
//! Module map (see the architecture overview in the repo for the full picture):
//!
//! - [`model`]   — the type-agnostic node/edge domain: IDs, references, nodes, edges.
//! - [`corpus`]  — reading the files: repo discovery, scanning, frontmatter, wikilinks.
//! - [`index`]   — the derived SQLite cache: build, query, integrity checks.
//! - [`search`]  — hybrid FTS + vector retrieval.
//! - [`embed`]   — pluggable, local-by-default embedding with a content-hash cache.
//! - [`git`]     — the provenance layer: repo root, HEAD, diffs, last-indexed commit.
//! - [`commands`]— one module per CLI command, each returning a typed [`output::Output`].
//! - [`output`]  — the returned unit: paths + IDs, rendered as human text or JSON.
//! - [`mcp`]     — the STDIO MCP server that re-exposes the read commands as tools.
//! - [`config`]  — the one committed file under `.vaire/`, `config.toml`.
//! - [`error`]   — [`error::VaireError`] and its mapping to documented exit codes.

pub mod cli;
pub mod commands;
pub mod config;
pub mod corpus;
pub mod embed;
pub mod error;
pub mod git;
pub mod index;
pub mod mcp;
pub mod model;
pub mod output;
pub mod search;

pub use error::{ExitCode, Result, VaireError};
