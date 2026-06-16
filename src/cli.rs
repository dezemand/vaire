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

    /// Path to the config file (default: <root>/.vaire/config.toml).
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

    /// Hybrid full-text + vector search over the corpus.
    Search {
        query: String,
        #[arg(long = "type")]
        type_filter: Option<String>,
        /// Restrict to records in a project (matches the `project:` field).
        #[arg(long)]
        scope: Option<String>,
        /// Max results (default: 10).
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Suggest existing node IDs a descriptor might refer to (lookup-before-reference).
    Suggest {
        descriptor: String,
        #[arg(long = "type")]
        type_filter: Option<String>,
        /// Max suggestions (default: 5).
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },

    /// Every unresolved reference ([[?...]]) currently in the corpus.
    Unresolved {
        #[arg(long = "type")]
        type_filter: Option<String>,
        #[arg(long)]
        scope: Option<String>,
    },

    // ---- maintain commands (NOT on the MCP surface) ----
    /// Scaffold a corpus: write .vaire/config.toml so the directory is discoverable.
    Init {
        /// Directory to initialize (default: current directory).
        path: Option<PathBuf>,
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
    },

    /// Run the integrity guards ID-based discovery enables.
    Check {
        /// Promote warnings (orphans, drift) to failures.
        #[arg(long)]
        strict: bool,
        /// Reindex the working tree first, then check uncommitted edits.
        #[arg(long)]
        working_tree: bool,
    },

    /// Report index state.
    Status,

    // ---- agent surface ----
    /// Start a STDIO MCP server exposing the read commands as tools.
    Mcp,
}

impl Command {
    /// Whether `--json` is meaningful for this command (read commands + index/check/
    /// status emit JSON; `mcp` does not).
    pub fn supports_json(&self) -> bool {
        !matches!(self, Command::Mcp)
    }
}
