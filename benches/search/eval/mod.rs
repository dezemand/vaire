//! Reusable evaluation library for the `vaire search` quality + latency benchmark
//! (issue #52: long spec documents outrank short, more relevant concept/principle nodes).
//!
//! This module is shared between `benches/search/main.rs` (the CLI/orchestrator, run via
//! `cargo bench --bench search`) and `tests/search_relevance.rs` (a fast sanity check over
//! the `public` corpus, run via `cargo test`), via:
//!
//! ```ignore
//! #[path = "../benches/search/eval/mod.rs"]
//! mod eval;
//! ```
//!
//! It is built ONLY on `vaire`'s stable public API (`vaire::search::search`,
//! `vaire::index::{db, build}`, `vaire::config::Config`, `vaire::corpus::Repo`,
//! `vaire::embed::{Embedder, from_user_config}`, `vaire::userconfig::UserConfig`), so the
//! harness keeps working unmodified as the search implementation changes on other
//! ranking-optimisation branches. See `benches/search/README.md` for the full contract.

pub mod corpus;
pub mod embedders;
pub mod metrics;
pub mod queries;
pub mod report;
pub mod runner;
