//! Authored configuration — `.vaire/config.toml` (cli.md §6).
//!
//! The one committed file under `.vaire/`; everything else there is derived and
//! gitignored. All keys are optional; the defaults make `vaire` work with no config.
//! Resolution order: `--config` path > `<root>/.vaire/config.toml` > built-in defaults.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VaireError};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Package identity (v0.2 `knowledge.toml`). `name` is a slug, declared not
    /// path-derived; `version` is semver. Both required in a real manifest — an empty
    /// `name` (the default) is how [`Self::validate`] detects a missing one.
    pub name: String,
    pub version: String,
    pub description: Option<String>,

    /// Where this package is authored — its source repository URL (registry.md §6.1).
    /// Rides the manifest into the artifact and the registry record, so a consumer can
    /// choose `vaire pull` (read-only artifact) or clone-and-PR (authoring). Optional,
    /// free-form; older CLIs ignore it (unknown manifest keys are not errors).
    pub repository: Option<String>,

    /// Dependencies on other packages: `name → "^MAJOR"` (the only legal constraint form).
    pub dependencies: BTreeMap<String, String>,

    /// Where to look (the typed `id:` is still what makes a file a node).
    pub include: Vec<String>,
    pub exclude: Vec<String>,

    /// The entity types this package **defines** (the `type:` field, also the ID prefix in
    /// `type:id`). Growable; an unlisted prefix is still indexed, but `vaire check` warns
    /// when `vocabulary_strict` is set. A manifest that omits `types` defines none — hence
    /// the field-level default (empty), distinct from `Config::default()`'s test vocabulary.
    #[serde(default)]
    pub types: Vec<String>,
    pub vocabulary_strict: bool,

    /// Scoping is **data-driven**: *any* node that carries the [`Self::scope_field`]
    /// frontmatter field gets the composed address `<container-id>/<type>:<local-id>`,
    /// regardless of type (cli.md §6.1). These two lists don't gate that behaviour — they
    /// are a **lint policy**: `vaire check` warns when a scoped node's type is not permitted.
    /// A type is permitted iff it matches `scoped_types_whitelist` and not
    /// `scoped_types_blacklist`. `"*"` matches any type. Defaults (`["*"]` / `[]`) permit
    /// everything, so no warnings unless a policy is set.
    pub scoped_types_whitelist: Vec<String>,
    pub scoped_types_blacklist: Vec<String>,

    /// The frontmatter field whose value (a container node's ID) supplies the scope
    /// prefix. Default `"scope"` — generic, so its value can name any container type
    /// (`scope: project:atlas`, `scope: org:some-firm`). Set to e.g. `"project"` to tie
    /// scoping to a specific relationship field.
    pub scope_field: String,

    /// The type carried by release records, and the directory they are written to
    /// (registry.md §3.2). Defaults `"release"` / `"releases"`.
    ///
    /// Conventions, not reserved words: a type name is package vocabulary, and a
    /// knowledge base whose own subject matter means something by "release" renames these
    /// rather than losing the word. The classifier excludes `release_type` from its diff,
    /// so whatever it names is invisible to version computation.
    pub release_type: String,
    pub release_dir: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
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
            // Empty name = "no manifest / missing"; validated on load, not here.
            name: String::new(),
            version: "0.1.0".to_string(),
            description: None,
            repository: None,
            dependencies: BTreeMap::new(),
            // `releases/**` is here so a package that never touched its globs can cut a
            // release and have the record it writes actually be part of the corpus.
            include: vec![
                "knowledge/**/*.md".into(),
                "projects/**/*.md".into(),
                "releases/**/*.md".into(),
            ],
            exclude: vec![
                "**/node_modules/**".into(),
                "**/drafts/**".into(),
                "**/archive/**".into(),
            ],
            types: [
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
            release_type: crate::model::version::DEFAULT_RELEASE_TYPE.to_string(),
            release_dir: crate::model::version::DEFAULT_RELEASE_DIR.to_string(),
            // Permit any type to be scoped; no lint policy by default.
            scoped_types_whitelist: vec!["*".to_string()],
            scoped_types_blacklist: Vec::new(),
            scope_field: "scope".to_string(),
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
    /// Load from `path`, or return defaults if it does not exist. A present file is parsed
    /// **and validated** (identity + dependency constraints); a missing one yields defaults
    /// unchecked (callers that require a manifest gate on its existence via discovery).
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text, &path.display().to_string())
    }

    /// Parse and validate a manifest from its text. `origin` labels errors (a path for
    /// [`Self::load`]; e.g. `knowledge.toml@HEAD` when reading the committed manifest).
    pub fn parse(text: &str, origin: &str) -> Result<Config> {
        let cfg: Config =
            toml::from_str(text).map_err(|e| VaireError::Config(format!("{origin}: {e}")))?;
        cfg.validate()
            .map_err(|e| VaireError::Config(format!("{origin}: {e}")))?;
        Ok(cfg)
    }

    /// Validate manifest identity and dependency constraints. Returns a human-readable
    /// reason on the first violation.
    fn validate(&self) -> std::result::Result<(), String> {
        if !is_slug(&self.name) {
            return Err(format!(
                "`name` must be a slug matching [a-z][a-z0-9-]* (got {:?})",
                self.name
            ));
        }
        if !is_semver(&self.version) {
            return Err(format!(
                "`version` must be semver MAJOR.MINOR.PATCH (got {:?})",
                self.version
            ));
        }
        for (dep, constraint) in &self.dependencies {
            if !is_caret_major(constraint) {
                return Err(format!(
                    "dependency `{dep}` constraint must be the `^MAJOR` form (got {constraint:?})"
                ));
            }
        }
        Ok(())
    }

    /// Whether a scoped node of `node_type` is permitted by the lint policy: it matches the
    /// whitelist and not the blacklist (`"*"` matches any type). Behaviour-neutral — used
    /// only by `vaire check`, never to decide whether a node is scoped.
    pub fn scoping_permitted(&self, node_type: &str) -> bool {
        let matches = |list: &[String]| list.iter().any(|t| t == "*" || t == node_type);
        matches(&self.scoped_types_whitelist) && !matches(&self.scoped_types_blacklist)
    }
}

/// A package/id slug: `[a-z][a-z0-9-]*`.
pub fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Semver `MAJOR.MINOR.PATCH`, each a non-empty run of ASCII digits.
fn is_semver(s: &str) -> bool {
    let mut parts = s.split('.');
    let valid = parts
        .by_ref()
        .take(3)
        .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .count();
    valid == 3 && parts.next().is_none()
}

/// The only legal dependency constraint form: `^MAJOR` (a caret then a non-empty run of
/// digits). Tighter pins or ranges are rejected — minor/patch never break references, so a
/// pin could only create churn (manifest.md §5).
pub fn is_caret_major(s: &str) -> bool {
    matches!(s.strip_prefix('^'), Some(rest) if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}
