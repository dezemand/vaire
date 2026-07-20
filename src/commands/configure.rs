//! `vaire configure` — set embedding settings and secrets in the global user config (M2).
//!
//! Machine/consumer settings (how *you* embed; later, registry auth) live in the per-user
//! config, not in any package manifest. This command writes `<config-home>/config.toml`
//! (non-secret settings) and `<config-home>/credentials.toml` (secrets, `600`). It operates
//! on an explicit config home so it is corpus-independent and testable.

use std::path::Path;

use crate::config::EmbeddingProvider;
use crate::error::{Result, VaireError};
use crate::output::ConfigureOutput;
use crate::userconfig::{self, UserConfig};

/// The settings a single `configure` invocation may change. All optional — only the provided
/// fields are updated; the rest are preserved.
#[derive(Debug, Default)]
pub struct ConfigureOpts {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub dimensions: Option<usize>,
    pub command: Option<String>,
    pub openai_key: Option<String>,
    pub base_url: Option<String>,
}

/// Apply `opts` to the user config under `home`, writing config + credentials.
pub fn run(home: &Path, opts: ConfigureOpts) -> Result<ConfigureOutput> {
    let mut cfg = UserConfig::load_from(home)?;

    if let Some(p) = &opts.provider {
        cfg.embeddings.provider = parse_provider(p)?;
    }
    if let Some(m) = opts.model {
        cfg.embeddings.embedding_model = m;
    }
    if let Some(d) = opts.dimensions {
        cfg.embeddings.dimensions = d;
    }
    if let Some(c) = opts.command {
        cfg.embeddings.command = c;
    }
    let config_path = cfg.save_to(home)?;

    // Secrets go to credentials.toml (600), never the config file.
    let mut credentials_set = Vec::new();
    if let Some(k) = &opts.openai_key {
        userconfig::save_credential_to(home, "OPENAI_API_KEY", k)?;
        credentials_set.push("OPENAI_API_KEY".to_string());
    }
    if let Some(u) = &opts.base_url {
        userconfig::save_credential_to(home, "OPENAI_BASE_URL", u)?;
        credentials_set.push("OPENAI_BASE_URL".to_string());
    }

    Ok(ConfigureOutput {
        config_path: config_path.display().to_string(),
        provider: provider_name(cfg.embeddings.provider).to_string(),
        dimensions: cfg.embeddings.dimensions,
        credentials_set,
    })
}

fn parse_provider(s: &str) -> Result<EmbeddingProvider> {
    match s {
        "local" => Ok(EmbeddingProvider::Local),
        "command" => Ok(EmbeddingProvider::Command),
        "openai" => Ok(EmbeddingProvider::OpenAi),
        other => Err(VaireError::Usage(format!(
            "unknown embeddings provider '{other}' (expected: local | command | openai)"
        ))),
    }
}

fn provider_name(p: EmbeddingProvider) -> &'static str {
    match p {
        EmbeddingProvider::Local => "local",
        EmbeddingProvider::Command => "command",
        EmbeddingProvider::OpenAi => "openai",
    }
}
