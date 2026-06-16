//! Authored configuration — `.vaire/config.toml` (cli.md §6).
//!
//! The one committed file under `.vaire/`; everything else there is derived and
//! gitignored. All keys are optional; the defaults make `vaire` work with no config.
//! Resolution order: `--config` path > `<root>/.vaire/config.toml` > built-in defaults.

use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, VaireError};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where to look (the typed `id:` is still what makes a file a node).
    pub include: Vec<String>,
    pub exclude: Vec<String>,

    /// ID-prefix vocabulary. Growable; an unlisted prefix is still indexed, but
    /// `vaire check` warns when `vocabulary_strict` is set.
    pub id_prefixes: Vec<String>,
    pub vocabulary_strict: bool,

    /// Types whose IDs are **scoped**: a node of one of these types that carries the
    /// [`Self::scope_field`] frontmatter field gets the composed address
    /// `<container-id>/<type>:<local-id>`. Empty = off (all IDs flat/global, the
    /// default). See cli.md §6.1.
    pub scoped_types: Vec<String>,

    /// The frontmatter field whose value (a container node's ID) supplies the scope
    /// prefix for scoped types. Default `"scope"` — generic, so its value can name any
    /// container type (`scope: project:atlas`, `scope: org:some-firm`). Set to e.g.
    /// `"project"` to tie scoping to a specific relationship field.
    pub scope_field: String,

    pub embeddings: EmbeddingConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    /// `"local"` (built-in), `"command"` (shell out), or `"openai"` (OpenAI API).
    pub provider: EmbeddingProvider,
    /// Command used when `provider = "command"`: receives texts on stdin, returns
    /// vectors. Empty otherwise.
    pub command: String,
    /// Model used when `provider = "openai"`, e.g. `text-embedding-3-small`.
    pub embedding_model: String,
    pub dimensions: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    Local,
    Command,
    #[serde(rename = "openai")]
    OpenAi,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            include: vec!["knowledge/**/*.md".into(), "projects/**/*.md".into()],
            exclude: vec![
                "**/node_modules/**".into(),
                "**/drafts/**".into(),
                "**/archive/**".into(),
            ],
            id_prefixes: [
                "person",
                "department",
                "method",
                "system",
                "event",
                "record",
                "project",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            vocabulary_strict: false,
            scoped_types: Vec::new(),
            scope_field: "scope".to_string(),
            embeddings: EmbeddingConfig::default(),
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            provider: EmbeddingProvider::Local,
            command: String::new(),
            embedding_model: "text-embedding-3-small".to_string(),
            dimensions: 384,
        }
    }
}

impl Config {
    /// Load from `path`, or return defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))
    }
}
