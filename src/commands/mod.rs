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
pub mod clean;
pub mod configure;
pub mod deps;
pub mod index;
pub mod init;
#[cfg(feature = "pack")]
pub mod pack;
pub mod pin;
#[cfg(feature = "pack")]
pub mod pull;
#[cfg(feature = "pack")]
pub mod push;
pub mod refs;
pub mod registry;
pub mod release;
pub mod render;
pub mod resolve;
pub mod search;
pub mod status;
pub mod suggest;
pub mod unresolved;
pub mod upgrade;
pub mod yank;

use std::path::PathBuf;

use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::error::Result;

/// Reject a bare id in a rootless session.
///
/// A bare `type:id` means "in this package", and outside one there is no this package —
/// so the address is not merely unfound but unaskable. Said plainly rather than answered
/// with "no node with id", which would suggest the node is missing.
pub(crate) fn require_qualified(ctx: &Ctx, id: &crate::model::id::NodeId) -> Result<()> {
    if ctx.is_rootless() && id.package().is_none() {
        return Err(crate::error::VaireError::Usage(format!(
            "'{id}' is a bare id and there is no package to read it against — qualify it as \
             `@<package>/{id}`, or run this inside a package (`vaire catalog list` shows \
             what this machine knows)"
        )));
    }
    Ok(())
}

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
    /// `--frozen`: answer only from the store (registry.md §6).
    frozen: bool,
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
            frozen: false,
        })
    }

    /// A **rootless** context: no package to stand in, scope taken from the catalog
    /// (cli.md §6.8). Built when a read command runs outside any package, and by
    /// `--all` from inside one.
    ///
    /// The catalog is read once here and the handle dropped immediately — Turso locks a
    /// database exclusively on open, so holding it for the life of a session (an `mcp`
    /// server runs for hours) would lock every other vaire process on the machine out of
    /// it. What the session keeps is the answer, not the connection.
    ///
    /// `repo` is a placeholder rooted at the vaire home: read commands never touch it, and
    /// the maintain commands that do are never rootless.
    pub fn rootless(home: PathBuf) -> Result<Ctx> {
        Ctx::rootless_with(home, None, false)
    }

    /// [`Ctx::rootless`], plus a package the catalog may not have heard of.
    ///
    /// The one caller that passes `Some` is `--all` from inside a package: that flag
    /// *widens* a closure query (cli.md §3.4), so the package you are standing in has to
    /// stay in scope. Reads never record sightings, so a checkout that has not yet been
    /// indexed, checked, or `catalog add`ed is genuinely absent from the catalog — and
    /// "search everything" quietly excluding *here* is the one result nobody would read as
    /// correct. Seeded last, so a catalogued path for the same name is not displaced.
    /// `frozen` is taken here rather than applied afterwards because a rootless session's
    /// *scope* changes under it, not merely its gate: the catalog is the index of working
    /// copies, so a frozen session neither consults it nor takes its machine-wide lock, and
    /// a store entry a working copy would otherwise displace has to stay visible — else the
    /// package would be invisible **and** the working copy refused, leaving a release the
    /// store actually holds unusable.
    pub fn rootless_with(
        home: PathBuf,
        also: Option<(String, PathBuf)>,
        frozen: bool,
    ) -> Result<Ctx> {
        let store = crate::store::Store::at(&home);
        if frozen {
            let workspace = std::cell::OnceCell::new();
            let ws = crate::workspace::Workspace::rootless(store.packages()).frozen(store);
            let _ = workspace.set(ws);
            return Ok(Ctx {
                repo: Repo::at(home.clone()),
                config: Config::default(),
                home,
                workspace,
                embedder: std::cell::OnceCell::new(),
                frozen,
            });
        }
        // A working copy **displaces** the store entry of the same name — it does not merely
        // come later in the list. `Workspace::rootless` keeps every distinct path per name
        // and `locate` refuses to choose between them, so appending both would turn the
        // two-worlds rule into an ambiguity error the user cannot even clear (`catalog rm`
        // does not reach a store path).
        //
        // Ambiguity *among working copies* is untouched: two checkouts declaring one name is
        // a real question about which you meant, and the store has no standing to settle it.
        let live = crate::catalog::Catalog::open(&home)?.live_packages()?;
        let claimed: std::collections::BTreeSet<&str> = live
            .iter()
            .chain(also.iter())
            .map(|(name, _)| name.as_str())
            .collect();
        let mut packages: Vec<(String, PathBuf)> = store
            .packages()
            .into_iter()
            .filter(|(name, _)| !claimed.contains(name.as_str()))
            .collect();
        packages.extend(live);
        packages.extend(also);
        let workspace = std::cell::OnceCell::new();
        let _ = workspace.set(crate::workspace::Workspace::rootless(packages));
        Ok(Ctx {
            repo: Repo::at(home.clone()),
            config: Config::default(),
            home,
            workspace,
            embedder: std::cell::OnceCell::new(),
            frozen: false,
        })
    }

    /// Whether this context has no package to stand in.
    pub fn is_rootless(&self) -> bool {
        self.workspace
            .get()
            .is_some_and(crate::workspace::Workspace::is_rootless)
    }

    /// Answer only from the store for this invocation (`--frozen`).
    ///
    /// Set before the workspace is built, which is why it is a builder rather than a
    /// setter: the memoized view carries the restriction, so a later `workspace()` cannot
    /// hand back an unrestricted one.
    pub fn with_frozen(mut self, frozen: bool) -> Ctx {
        self.frozen = frozen;
        // A rootless context builds its workspace in the constructor, so flipping the flag
        // afterwards would leave an unrestricted view behind it — silently unfrozen in
        // exactly the mode agents and CI run in. Re-wrap what is already there.
        if frozen && let Some(ws) = self.workspace.take() {
            let _ = self
                .workspace
                .set(ws.frozen(crate::store::Store::at(&self.home)));
        }
        self
    }

    /// Whether this invocation answers only from the store.
    pub fn is_frozen(&self) -> bool {
        self.frozen
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
            let mut ws = crate::workspace::Workspace::new(&self.repo, &self.config)?;
            if self.frozen {
                ws = ws.frozen(crate::store::Store::at(&self.home));
            }
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
