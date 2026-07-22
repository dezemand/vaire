//! `vaire configure` — set machine/consumer settings in the global user config (M2).
//!
//! Two surfaces over the same apply logic:
//!   * `vaire configure embeddings [--provider … --api-key-stdin]` — non-interactive; [`run`].
//!   * `vaire configure` (no section) — an interactive, guided prompt; [`run_interactive`].
//!
//! Machine/consumer settings (how *you* embed; later, registry auth) live in the per-user
//! config, not in any package manifest. This command writes `<config-home>/config.toml`
//! (non-secret settings) and `<config-home>/credentials.toml` (secrets, `600`). It operates
//! on an explicit config home so it is corpus-independent and testable.

use std::path::Path;

use inquire::error::InquireError;
use inquire::{CustomType, Password, PasswordDisplayMode, Select, Text};

use crate::config::EmbeddingProvider;
use crate::error::{Result, VaireError};
use crate::output::ConfigureOutput;
use crate::userconfig::{self, UserConfig};

/// The settings a single `configure embeddings` invocation may change. All optional — only
/// the provided fields are updated; the rest are preserved.
#[derive(Debug, Default)]
pub struct ConfigureOpts {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub dimensions: Option<usize>,
    pub command: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
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
    if let Some(k) = &opts.api_key {
        userconfig::save_credential_to(home, "OPENAI_API_KEY", k)?;
        credentials_set.push("OPENAI_API_KEY".to_string());
    }
    if let Some(u) = &opts.api_url {
        userconfig::save_credential_to(home, "OPENAI_BASE_URL", u)?;
        credentials_set.push("OPENAI_BASE_URL".to_string());
    }

    Ok(ConfigureOutput {
        config_path: config_path.display().to_string(),
        section: "embeddings".to_string(),
        provider: provider_name(cfg.embeddings.provider).to_string(),
        dimensions: cfg.embeddings.dimensions,
        credentials_set,
        cancelled: false,
        local_packages: display_local(&cfg),
    })
}

/// `vaire configure local-packages [<path>] [--unset]` — set, clear, or show the
/// local-packages root (cli.md §6.3). The path is stored **canonical**, so a link
/// materialized from it is stable even if the caller passed a relative or symlinked path.
pub fn run_local_packages(home: &Path, path: Option<&str>, unset: bool) -> Result<ConfigureOutput> {
    let mut cfg = UserConfig::load_from(home)?;

    let config_path = match (path, unset) {
        (Some(_), true) => {
            return Err(VaireError::Usage(
                "pass a path or --unset, not both".to_string(),
            ));
        }
        (Some(p), false) => {
            let given = expand_tilde(p);
            // Reject up front rather than storing a root that discovers nothing: a typo
            // here would otherwise surface much later as "dependency not found".
            let root = std::fs::canonicalize(&given)
                .map_err(|e| VaireError::Usage(format!("{}: {e}", given.display())))?;
            if !root.is_dir() {
                return Err(VaireError::Usage(format!(
                    "{} is not a directory",
                    root.display()
                )));
            }
            cfg.packages.local = Some(root);
            cfg.save_to(home)?
        }
        (None, true) => {
            cfg.packages.local = None;
            cfg.save_to(home)?
        }
        // Neither: report the current setting, writing nothing.
        (None, false) => home.join("config.toml"),
    };

    Ok(ConfigureOutput {
        config_path: config_path.display().to_string(),
        section: "local-packages".to_string(),
        provider: provider_name(cfg.embeddings.provider).to_string(),
        dimensions: cfg.embeddings.dimensions,
        credentials_set: Vec::new(),
        cancelled: false,
        local_packages: display_local(&cfg),
    })
}

fn display_local(cfg: &UserConfig) -> Option<String> {
    cfg.packages.local.as_ref().map(|p| p.display().to_string())
}

/// Expand a leading `~/`, so a quoted `"~/Documents/Knowledge"` (which the shell leaves
/// alone) behaves like the unquoted form.
fn expand_tilde(p: &str) -> std::path::PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(base) = directories::BaseDirs::new()
    {
        return base.home_dir().join(rest);
    }
    std::path::PathBuf::from(p)
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

/// Interactive `vaire configure` (no section): pick a section, then walk its settings
/// through guided prompts, seeded with the current values. Returns `Ok(None)` if the user
/// cancels (Esc / Ctrl-C) at any step — a clean exit, not an error.
pub fn run_interactive(home: &Path) -> Result<ConfigureOutput> {
    let cfg = UserConfig::load_from(home)?;

    let Some(section) =
        cancellable(Select::new("Configure", vec!["Embeddings", "Local packages"]).prompt())?
    else {
        return Ok(cancelled(home));
    };
    match section {
        "Embeddings" => configure_embeddings_interactively(home, &cfg),
        "Local packages" => configure_local_packages_interactively(home, &cfg),
        other => Err(VaireError::Usage(format!("unknown section '{other}'"))),
    }
}

/// The local-packages branch of the interactive flow: one prompt, seeded with the current
/// root. An empty answer clears the setting.
fn configure_local_packages_interactively(
    home: &Path,
    cfg: &UserConfig,
) -> Result<ConfigureOutput> {
    let current = display_local(cfg).unwrap_or_default();
    let Some(answer) = cancellable(
        Text::new("Local packages directory (blank to unset)")
            .with_default(&current)
            .with_help_message("where your local packages live; searched by declared name")
            .prompt(),
    )?
    else {
        return Ok(cancelled(home));
    };
    let answer = answer.trim();
    if answer.is_empty() {
        return run_local_packages(home, None, true);
    }
    run_local_packages(home, Some(answer), false)
}

/// The embeddings branch of the interactive flow. Builds a [`ConfigureOpts`] from prompts
/// (defaulting to the current config) and hands off to [`run`].
fn configure_embeddings_interactively(home: &Path, cfg: &UserConfig) -> Result<ConfigureOutput> {
    let providers = vec!["local", "openai", "command"];
    let start = provider_name(cfg.embeddings.provider);
    let cursor = providers.iter().position(|p| *p == start).unwrap_or(0);

    let Some(provider) = cancellable(
        Select::new("Embedding provider", providers)
            .with_starting_cursor(cursor)
            .prompt(),
    )?
    else {
        return Ok(cancelled(home));
    };

    let mut opts = ConfigureOpts {
        provider: Some(provider.to_string()),
        ..Default::default()
    };

    // Provider-specific settings. Blank answers keep the current value.
    match provider {
        "openai" => {
            let model = cfg.embeddings.embedding_model.clone();
            let Some(m) = cancellable(Text::new("Embedding model").with_default(&model).prompt())?
            else {
                return Ok(cancelled(home));
            };
            opts.model = Some(m);

            let Some(dims) = prompt_dimensions(cfg)? else {
                return Ok(cancelled(home));
            };
            opts.dimensions = dims;

            // Secrets: masked, optional. Skipped or blank → leave credentials.toml untouched.
            let Some(key) = cancellable(
                Password::new("API key (blank to keep current)")
                    .with_display_mode(PasswordDisplayMode::Masked)
                    .without_confirmation()
                    .with_help_message("stored in credentials.toml (owner-only), never the config")
                    .prompt_skippable(),
            )?
            else {
                return Ok(cancelled(home));
            };
            opts.api_key = key.filter(|s| !s.is_empty());

            let Some(url) = cancellable(
                Text::new("API base URL (optional, blank to keep current)").prompt_skippable(),
            )?
            else {
                return Ok(cancelled(home));
            };
            opts.api_url = url.filter(|s| !s.is_empty());
        }
        "command" => {
            let cmd = cfg.embeddings.command.clone();
            let Some(c) = cancellable(Text::new("Embedder command").with_default(&cmd).prompt())?
            else {
                return Ok(cancelled(home));
            };
            opts.command = Some(c);

            let Some(dims) = prompt_dimensions(cfg)? else {
                return Ok(cancelled(home));
            };
            opts.dimensions = dims;
        }
        _ => {
            // local: only the vector width matters.
            let Some(dims) = prompt_dimensions(cfg)? else {
                return Ok(cancelled(home));
            };
            opts.dimensions = dims;
        }
    }

    run(home, opts)
}

/// Prompt for embedding dimensions, defaulting to the current value. The outer `Option`
/// distinguishes cancellation (`None`) from the always-present answer (`Some(Some(_))`).
#[allow(clippy::type_complexity)]
fn prompt_dimensions(cfg: &UserConfig) -> Result<Option<Option<usize>>> {
    let answer = cancellable(
        CustomType::<usize>::new("Embedding dimensions")
            .with_default(cfg.embeddings.dimensions)
            .prompt(),
    )?;
    Ok(answer.map(Some))
}

/// The outcome printed when the user cancels: report the config path unchanged.
fn cancelled(home: &Path) -> ConfigureOutput {
    let cfg = UserConfig::load_from(home).unwrap_or_default();
    ConfigureOutput {
        config_path: home.join("config.toml").display().to_string(),
        section: "embeddings".to_string(),
        provider: provider_name(cfg.embeddings.provider).to_string(),
        dimensions: cfg.embeddings.dimensions,
        credentials_set: Vec::new(),
        cancelled: true,
        local_packages: display_local(&cfg),
    }
}

/// Fold inquire's cancel/interrupt errors into `Ok(None)`; surface anything else as a usage
/// error. A missing TTY points the user at the non-interactive form.
fn cancellable<T>(r: std::result::Result<T, InquireError>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(InquireError::NotTTY) => Err(VaireError::Usage(
            "`vaire configure` needs an interactive terminal; \
             use `vaire configure embeddings --provider … --api-key-stdin` instead"
                .into(),
        )),
        Err(e) => Err(VaireError::Usage(format!("interactive prompt failed: {e}"))),
    }
}
