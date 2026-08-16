//! The `vaire` command-line surface (cli.md §2).
//!
//! clap derive definitions: the global flags and the two classes of subcommand. This
//! is *only* the parse layer — each variant dispatches into [`crate::commands`]. The
//! MCP server (`vaire mcp`) re-exposes the read subcommands, so there is exactly one
//! implementation behind both surfaces (cli.md §1).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "vaire",
    version,
    about = "Derived reference-graph index over a Markdown corpus"
)]
pub struct Cli {
    // ---- global flags (cli.md §2.2) ----
    /// Corpus repo root. Overrides discovery and VAIRE_REPO.
    #[arg(long, global = true, env = "VAIRE_REPO")]
    pub repo: Option<PathBuf>,

    /// Emit JSON instead of human-readable text. Read commands only.
    #[arg(long, global = true)]
    pub json: bool,

    /// Path to the manifest (default: <root>/knowledge.toml).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Suppress progress and non-essential output.
    #[arg(long, short, global = true)]
    pub quiet: bool,

    /// Extra diagnostics on stderr. Repeatable.
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Disable ANSI color (also honored via NO_COLOR).
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Answer only from the store: refuse a dependency that resolves to a working copy,
    /// whose version nobody else can obtain. The expected mode for agents and CI, where
    /// "answered against acme-core 1.4.2" has to be a claim someone can check.
    #[arg(long, global = true)]
    pub frozen: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    // ---- read commands (MCP-exposed) ----
    /// Resolve a node ID to its location and frontmatter.
    Resolve { id: String },

    /// Render a node as portable Markdown: frontmatter kept, wikilinks resolved to links.
    Render { id: String },

    /// Nodes that reference <id> (inbound edges).
    Backlinks {
        id: String,
        /// Restrict to referencing nodes of a given type.
        #[arg(long = "type")]
        type_filter: Option<String>,
        /// Cap results (default: unbounded).
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Nodes that <id> references (outbound edges).
    Refs {
        id: String,
        /// Traverse outbound edges N hops (default: 1).
        #[arg(long, default_value_t = 1)]
        depth: u32,
        #[arg(long = "type")]
        type_filter: Option<String>,
    },

    /// Hybrid full-text + vector search over the corpus and its linked dependencies.
    Search {
        query: String,
        #[arg(long = "type")]
        type_filter: Option<String>,
        /// Restrict to records in a container (`project:atlas`, or `@pkg/project:atlas`
        /// to search inside a dependency's container).
        #[arg(long)]
        scope: Option<String>,
        /// Max results (default: 10).
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Search only this package (skip linked dependencies).
        #[arg(long, conflicts_with = "all")]
        local: bool,
        /// Search every package in the catalog, not just this one's dependencies.
        /// Implied when run outside a package.
        #[arg(long)]
        all: bool,
    },

    /// Suggest existing node IDs a descriptor might refer to (lookup-before-reference).
    Suggest {
        descriptor: String,
        #[arg(long = "type")]
        type_filter: Option<String>,
        /// Max suggestions (default: 5).
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Suggest only from this package (skip linked dependencies).
        #[arg(long, conflicts_with = "all")]
        local: bool,
        /// Suggest from every package in the catalog, not just this one's dependencies.
        /// Implied when run outside a package.
        #[arg(long)]
        all: bool,
    },

    /// Print the resolved local dependency tree (live link inspection; no index needed).
    Deps,

    /// Every unresolved reference ([[?...]]) currently in the corpus.
    Unresolved {
        #[arg(long = "type")]
        type_filter: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        /// Also list linked dependencies' loose ends (default: this package only —
        /// a dependency's worklist belongs to its owner).
        #[arg(long = "all-packages")]
        all_packages: bool,
    },

    // ---- maintain commands (NOT on the MCP surface) ----
    /// Scaffold a package: write knowledge.toml so the directory is discoverable
    /// (or migrate a legacy .vaire/config.toml).
    Init {
        /// Directory to initialize (default: current directory).
        path: Option<PathBuf>,
    },

    /// Declare a dependency on another package in knowledge.toml.
    Add {
        /// The package to depend on: `<name>` or `<name>@^MAJOR` (default `^1`).
        spec: String,
        /// Also link where it lives: creates the `.vaire/packages/<name>` symlink to
        /// this path (a package directory declaring the same name).
        #[arg(long)]
        link: Option<PathBuf>,
        /// Do not record this package in the catalog. Skips, never forgets.
        #[arg(long = "no-register")]
        no_register: bool,
    },

    /// (Re)build the index from the committed files.
    Index {
        /// Cold rebuild: drop and recreate the index, re-embed everything.
        #[arg(long)]
        full: bool,
        /// Index the working tree (uncommitted edits) instead of the committed tree.
        #[arg(long)]
        working_tree: bool,
        /// Re-embed every section with the current provider, bypassing the cache (use
        /// after changing the embedding model/provider). Keeps the graph as-is.
        #[arg(long = "re-embed")]
        re_embed: bool,
        /// Skip the linked-dependency ensure pass (index only this package).
        #[arg(long = "no-deps")]
        no_deps: bool,
        /// Do not record this package in the catalog. Skips, never forgets — an existing
        /// sighting is left exactly as it was.
        #[arg(long = "no-register")]
        no_register: bool,
    },

    /// Run the integrity guards ID-based discovery enables.
    Check {
        /// Promote warnings (orphans, drift) to failures.
        #[arg(long)]
        strict: bool,
        /// Reindex the working tree first, then check uncommitted edits.
        #[arg(long)]
        working_tree: bool,
        /// Skip the linked-dependency ensure pass (resolution lints then judge the
        /// dependency indexes as-is).
        #[arg(long = "no-deps")]
        no_deps: bool,
        /// Do not record this package in the catalog. Skips, never forgets.
        #[arg(long = "no-register")]
        no_register: bool,
    },

    /// Report index state.
    Status,

    /// Cut a release: classify what changed since the last one, compute the version,
    /// write the manifest and a release record, commit, and tag. Never pushes.
    Release {
        /// Consent to a MAJOR — required when the classifier sees one (removed or
        /// retired entities), and enough on its own to escalate a small edit that
        /// reverses a truth the classifier cannot see.
        #[arg(long)]
        major: bool,
        /// Classify and report what would happen; write nothing.
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Accept advisory prompts without asking (the CI posture).
        #[arg(long, short = 'y')]
        yes: bool,
        /// File holding the invalidated-assumptions notes a MAJOR must carry — what
        /// dependents read to decide whether their references still hold.
        #[arg(long)]
        notes: Option<PathBuf>,
        /// Release from a branch that is not the repository's mainline.
        #[arg(long = "allow-branch")]
        allow_branch: bool,
        /// Publish the release to a registry once it is cut. A convenience over the
        /// `release`/`push` split, never a merge of it: the tag exists either way, so a
        /// failed upload is retried with `vaire push` and costs nothing.
        #[arg(long)]
        push: bool,
        /// Reserved: back-patch an older major line.
        #[arg(long, hide = true)]
        onto: Option<String>,
    },

    /// Build this package's distributable artifact from the committed tree
    /// (`.vaire/dist/<name>-<version>.tgz`: manifest, corpus files, every file
    /// they reference, pre-built index).
    #[cfg(feature = "pack")]
    Pack {
        /// Strip section vectors from the shipped index — smaller, and byte-reproducible
        /// regardless of embedding provider.
        #[arg(long = "no-embeddings")]
        no_embeddings: bool,
    },

    /// Self-update: download the release binary for this platform and replace this
    /// executable (same contract as the installer script).
    Upgrade {
        /// Version to install (e.g. `0.3.0`; default: the latest release, only if newer.
        /// An explicit version always installs, so `vaire upgrade <current>` repairs an
        /// install).
        version: Option<String>,
        /// Only report whether a newer release exists; install nothing.
        #[arg(long)]
        check: bool,
    },

    /// What packages this machine knows and where they live.
    Catalog {
        #[command(subcommand)]
        action: CatalogAction,
    },

    /// Fetch a release into the store: verify it, unpack it, rebuild its index here, and
    /// seal it read-only. Nothing else in the tool ever downloads a package.
    #[cfg(feature = "pack")]
    Pull {
        /// `<name>`, `<name>@^MAJOR`, or `<name>@<version>` for an exact one (including a
        /// yanked one). Default: every declared dependency this machine cannot satisfy.
        spec: Option<String>,
        /// Only ask this registry. Default: every configured one, in priority order.
        #[arg(long)]
        registry: Option<String>,
        /// Reproduce `knowledge.lock` exactly — the versions it names, checked against the
        /// digests it records. Takes no package name.
        #[arg(long, conflicts_with = "spec")]
        locked: bool,
        /// Report what would be fetched; write nothing.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Upload released versions to a registry. Idempotent: what is already published is
    /// skipped, and every artifact is rebuilt from its own release tag, so this works from
    /// a fresh clone.
    #[cfg(feature = "pack")]
    Push {
        /// Publish only this version (default: every release tag the registry lacks).
        version: Option<String>,
        /// Which configured registry. Required only when several are configured and none
        /// has the highest priority.
        #[arg(long)]
        registry: Option<String>,
        /// Who may see and fetch this package here: open | restricted | unlisted. Sticky —
        /// omitting it keeps whatever the registry already records.
        #[arg(long)]
        access: Option<String>,
        /// Where to ask for access, carried verbatim in the refusal a restricted package
        /// produces (e.g. "request via #team-powertrain-knowledge").
        #[arg(long = "access-hint")]
        access_hint: Option<String>,
        /// Report what would be published; upload nothing.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Mark a published release as one nobody should newly adopt. The artifact stays put,
    /// so anything already pinned to it keeps resolving.
    Yank {
        /// `<name>@<version>`, e.g. `acme-core@1.4.2`.
        spec: String,
        #[arg(long)]
        registry: Option<String>,
        /// Clear the flag instead of setting it.
        #[arg(long)]
        undo: bool,
    },

    /// Hold a dependency at one exact version: it survives retention and `vaire clean`,
    /// and resolution takes it over anything newer in the same major line.
    Pin {
        /// `<name>@<version>`, e.g. `acme-core@1.4.2`. The version must already be in the
        /// store — a pin records the digest of the artifact it holds.
        spec: String,
    },

    /// Release a hold, so the dependency resolves to the highest satisfying version again.
    Unpin {
        /// The package to stop holding.
        name: String,
    },

    /// Remove store entries nothing needs: what no lockfile names, no pin holds, and
    /// nobody asked for by name. Everything removed is still published.
    Clean {
        /// Stop holding this package, then sweep. Its locked and pinned versions survive.
        package: Option<String>,
        /// Report what would go; delete nothing.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// The remote registries this machine publishes to and pulls from.
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },

    /// Configure global user settings. With no subcommand, opens an interactive prompt.
    Configure {
        #[command(subcommand)]
        section: Option<ConfigureSection>,
    },

    // ---- agent surface ----
    /// Start a STDIO MCP server exposing the read commands as tools.
    Mcp,
}

impl Command {
    /// Whether this is a read command — the class that can run without a package to
    /// stand in, against the catalog (cli.md §6.8).
    ///
    /// The test is "does this question still mean something with no package to stand in?",
    /// not "does this mutate?". `unresolved` qualifies because it already has a widened
    /// form (`--all-packages`), which is what it takes rootless. `deps` does not and is
    /// deliberately absent: it reports one package's link tree, so outside a package the
    /// honest answer is *no corpus found* rather than a tree belonging to nobody. `mcp` is
    /// in the class too, but is dispatched before this is consulted.
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            Command::Resolve { .. }
                | Command::Render { .. }
                | Command::Backlinks { .. }
                | Command::Refs { .. }
                | Command::Search { .. }
                | Command::Suggest { .. }
                | Command::Unresolved { .. }
        )
    }

    /// Whether the command asked for the whole catalog rather than this package's
    /// closure (`--all`).
    pub fn wants_all(&self) -> bool {
        matches!(
            self,
            Command::Search { all: true, .. } | Command::Suggest { all: true, .. }
        )
    }

    /// Whether `--json` is meaningful for this command (read commands + index/check/
    /// status emit JSON; `mcp` does not).
    pub fn supports_json(&self) -> bool {
        !matches!(self, Command::Mcp)
    }
}

/// Managing the catalog by hand. Most of it fills itself — `index`, `check`, and `add`
/// record what they touch — so these cover what ambience cannot reach.
#[derive(Debug, Subcommand)]
pub enum CatalogAction {
    /// Record a package explicitly (default: the current directory).
    Add { path: Option<PathBuf> },

    /// Forget a package: by path, by declared name, or every sighting whose path no
    /// longer answers.
    Rm {
        /// A path or a package name.
        target: Option<String>,
        /// Forget every sighting whose path no longer answers.
        #[arg(long)]
        missing: bool,
    },

    /// List every package the catalog knows, re-checking whether each path still answers.
    List,

    /// Walk a directory once and record every package under it — the bulk import for a
    /// tree you already have.
    Scan { dir: PathBuf },
}

/// Configuring remotes. A noun group, like `catalog` — which is what keeps the three
/// "add"s unambiguous: `add` is a dependency, `catalog add` a package on this machine,
/// `registry add` a remote.
#[derive(Debug, Subcommand)]
pub enum RegistryAction {
    /// Configure a registry. The location may be a URL or a directory (`./registry`),
    /// which is all a static registry ever is.
    Add {
        name: String,
        url: String,
        /// Fan-out order, and the tiebreaker when a command that needs one registry is not
        /// told which. Higher first.
        #[arg(long, default_value_t = 0)]
        priority: i64,
        /// Keep this registry out of `vaire search`'s default fan-out.
        #[arg(long = "no-search")]
        no_search: bool,
    },

    /// Every configured registry.
    List,

    /// Stop asking a registry. Nothing already pulled from it is forgotten.
    Rm { name: String },

    /// What a registry is, what it can do, and what it holds.
    Show { name: String },
}

/// The sections `vaire configure <section>` can set non-interactively. Bare `vaire
/// configure` (no section) walks the same settings through an interactive prompt.
#[derive(Debug, Subcommand)]
pub enum ConfigureSection {
    /// Configure the embedding provider and its credentials.
    Embeddings {
        /// Embedding provider: local | command | openai.
        #[arg(long)]
        provider: Option<String>,
        /// Embedding model (for the openai provider).
        #[arg(long)]
        model: Option<String>,
        /// Embedding vector dimensions.
        #[arg(long)]
        dimensions: Option<usize>,
        /// Command to run for the `command` provider.
        #[arg(long)]
        command: Option<String>,
        /// Read an API key from standard input and store it in credentials.toml. This avoids
        /// exposing it in shell history or process arguments.
        #[arg(long = "api-key-stdin")]
        api_key_stdin: bool,
        /// API base URL override. Stored in credentials.toml.
        #[arg(long = "api-url")]
        api_url: Option<String>,
    },
}
