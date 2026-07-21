//! Global per-user configuration (v0.2.0 M2).
//!
//! Machine/consumer settings — how *you* embed, and later how you authenticate to a
//! registry — are not part of any package's manifest (`knowledge.toml`). They live in a
//! per-user config at `<config-home>/config.toml`, with secrets kept separately in
//! `credentials.toml` (see [`crate::userconfig::credentials`], a later step).
//!
//! The config home is `VAIRE_CONFIG_HOME` if set, else the platform config directory for
//! `vaire` (`~/.config/vaire` on Linux, `~/Library/Application Support/vaire` on macOS,
//! `%APPDATA%\vaire` on Windows). Load/save take an explicit home so callers (and tests)
//! can inject one without touching the environment.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::EmbeddingConfig;
use crate::error::{Result, VaireError};

/// The global user config. Currently just embeddings; `[registry]` auth lands later.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct UserConfig {
    pub embeddings: EmbeddingConfig,
}

impl UserConfig {
    /// Resolve the config home ([`config_home`]) and load `config.toml`.
    pub fn load() -> Result<UserConfig> {
        Self::load_from(&config_home())
    }

    /// Load `<home>/config.toml`, or defaults if it does not exist.
    pub fn load_from(home: &Path) -> Result<UserConfig> {
        let path = home.join("config.toml");
        if !path.exists() {
            return Ok(UserConfig::default());
        }
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text).map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))
    }

    /// Write to `<config-home>/config.toml`. Returns the path written.
    pub fn save(&self) -> Result<PathBuf> {
        self.save_to(&config_home())
    }

    /// Write to `<home>/config.toml`, creating `home` if needed. Returns the path written.
    pub fn save_to(&self, home: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(home)?;
        let path = home.join("config.toml");
        let text = toml::to_string_pretty(self)
            .map_err(|e| VaireError::Config(format!("serialize user config: {e}")))?;
        std::fs::write(&path, text)?;
        Ok(path)
    }
}

/// The user config home: `VAIRE_CONFIG_HOME` if set and non-empty, else the platform config
/// directory for `vaire`. Falls back to `.vaire-config` in the cwd only if the platform
/// directory cannot be determined (no home dir).
pub fn config_home() -> PathBuf {
    if let Ok(dir) = std::env::var("VAIRE_CONFIG_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("", "", "vaire")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".vaire-config"))
}

/// Resolve a secret (e.g. `OPENAI_API_KEY`): an existing **environment variable wins**,
/// otherwise it is read from `<config-home>/credentials.toml`. Returns `None` if in neither.
pub fn credential(key: &str) -> Option<String> {
    credential_from(key, &config_home())
}

/// [`credential`] against an explicit config home (the DI seam for tests).
pub fn credential_from(key: &str, home: &Path) -> Option<String> {
    if let Ok(v) = std::env::var(key)
        && !v.is_empty()
    {
        return Some(v);
    }
    let path = home.join("credentials.toml");
    // An absent file is the normal "not configured" case → silent `None`. A file that exists
    // but does not parse is different: warn, so a corrupt file doesn't masquerade as "no key".
    let text = std::fs::read_to_string(&path).ok()?;
    let table: toml::Table = match toml::from_str(&text) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "warning: {}: {e} (ignoring credentials file)",
                path.display()
            );
            return None;
        }
    };
    table.get(key)?.as_str().map(str::to_string)
}

/// Store `key = value` in `<config-home>/credentials.toml`, merging with any existing keys.
pub fn save_credential(key: &str, value: &str) -> Result<PathBuf> {
    save_credential_to(&config_home(), key, value)
}

/// [`save_credential`] against an explicit config home. The file is written with `600`
/// permissions on Unix (owner read/write only), since it holds secrets.
pub fn save_credential_to(home: &Path, key: &str, value: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(home)?;
    let path = home.join("credentials.toml");

    // Merge into any existing table so we don't clobber other secrets.
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))?,
        Err(_) => toml::Table::new(),
    };
    table.insert(key.to_string(), toml::Value::String(value.to_string()));
    let text = toml::to_string_pretty(&table)
        .map_err(|e| VaireError::Config(format!("serialize credentials: {e}")))?;
    write_private(&path, &text)?;
    Ok(path)
}

/// Write `text` to a secrets file, creating it with `600` (owner read/write only) from the
/// start on Unix — so it is never briefly group/world-readable under a permissive umask in the
/// gap between a plain `write` and a follow-up `chmod`. An existing file is truncated,
/// re-restricted to `600`, then rewritten, so the secret bytes are only ever present at `600`.
#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600) // honored only when the file is newly created…
        .open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?; // …so fix up a pre-existing one
    f.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text)?;
    Ok(())
}
