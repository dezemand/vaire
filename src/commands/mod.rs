//! Commands — one module per CLI subcommand (cli.md §3–§4).
//!
//! Read commands (`resolve`, `backlinks`, `refs`, `search`, `unresolved`) are pure
//! queries against the built index and are re-exposed over MCP. Maintain commands
//! (`index`, `check`, `status`) build/validate/report and are **not** on the MCP
//! surface. Each `run` returns a typed output; the binary handles rendering (human vs
//! `--json`) and the exit-code mapping (cli.md §7).

pub mod add;
pub mod backlinks;
pub mod catalog;
pub mod check;
pub mod configure;
pub mod deps;
pub mod index;
pub mod init;
#[cfg(feature = "pack")]
pub mod pack;
pub mod refs;
pub mod release;
pub mod render;
pub mod resolve;
pub mod search;
pub mod status;
pub mod suggest;
pub mod unresolved;
pub mod upgrade;

use std::path::PathBuf;

use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::error::Result;

/// Resolved per-invocation context shared by every command: the located repo and the
/// loaded config. Built once from the global flags before dispatch.
pub struct Ctx {
    pub repo: Repo,
    pub config: Config,
    /// The vaire home holding the catalog. Resolved once per invocation rather than read
    /// from the environment at each use, so a test (or a future `--home`) can point one
    /// run at its own catalog without touching process-global state.
    home: PathBuf,
    /// The linked-package view (cli.md §6.5), built lazily on first cross-package need —
    /// a standalone package never constructs it.
    workspace: std::cell::OnceCell<crate::workspace::Workspace>,
    /// The embedding provider can own an HTTP connection pool or command configuration;
    /// retain it for the invocation (and all MCP requests) rather than recreating it per call.
    embedder: std::cell::OnceCell<Box<dyn crate::embed::Embedder>>,
}

impl Ctx {
    /// Resolve the repo (cli.md §2.1) and load config (cli.md §6) from the global flags.
    pub fn new(repo_override: Option<PathBuf>, config_override: Option<PathBuf>) -> Result<Ctx> {
        let cwd = std::env::current_dir()?;
        let repo = Repo::discover(repo_override.as_deref(), &cwd)?;
        let config_path = config_override.unwrap_or_else(|| repo.config_path());
        let config = Config::load(&config_path)?;
        Ok(Ctx {
            repo,
            config,
            home: crate::userconfig::vaire_home(),
            workspace: std::cell::OnceCell::new(),
            embedder: std::cell::OnceCell::new(),
        })
    }

    /// Point this invocation at a different vaire home. The seam in-process tests use to
    /// get their own catalog: the alternative — one process-global environment variable —
    /// would have every test in a binary sharing (and serializing on) one catalog file.
    pub fn with_home(mut self, home: PathBuf) -> Ctx {
        self.home = home;
        self
    }

    /// The vaire home this invocation reads and writes the catalog in.
    pub fn home(&self) -> &std::path::Path {
        &self.home
    }

    /// The linked-package view rooted at this package (memoized).
    pub fn workspace(&self) -> Result<&crate::workspace::Workspace> {
        if self.workspace.get().is_none() {
            let ws = crate::workspace::Workspace::new(&self.repo, &self.config)?;
            let _ = self.workspace.set(ws);
        }
        Ok(self.workspace.get().expect("just initialized"))
    }

    /// Open the already-built index, mapping a missing/corrupt file to the documented
    /// exit codes. Read commands never build it as a side effect (cli.md §1). An index
    /// whose schema version doesn't match this binary is rejected as corrupt (exit `3`),
    /// directing the user to rebuild — rather than querying an unexpected shape.
    pub fn open_index(&self) -> Result<crate::index::Index> {
        use crate::index::db::SCHEMA_VERSION;
        let index = crate::index::Index::open(&self.repo.index_db())?;
        match index.schema_version() {
            Some(v) if v == SCHEMA_VERSION => Ok(index),
            other => Err(crate::error::VaireError::IndexCorrupt(format!(
                "index schema version {} is incompatible with this vaire (expects {SCHEMA_VERSION}); \
                 rebuild with `vaire index --full`",
                other.map_or_else(|| "unknown".to_string(), |v| v.to_string()),
            ))),
        }
    }

    /// Build the embedder from the global user config (M2); providers that need secrets
    /// (e.g. OpenAI) resolve them from the environment or `credentials.toml`.
    pub fn embedder(&self) -> Result<&dyn crate::embed::Embedder> {
        if self.embedder.get().is_none() {
            let user = crate::userconfig::UserConfig::load()?;
            let embedder = crate::embed::from_user_config(&user)?;
            let _ = self.embedder.set(embedder);
        }
        Ok(self.embedder.get().expect("embedder initialized").as_ref())
    }
}
