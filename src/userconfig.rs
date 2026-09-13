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

/// The global user config: embeddings and where local packages live; `[registry]` auth
/// lands later.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct UserConfig {
    pub embeddings: EmbeddingConfig,
    pub packages: PackagesConfig,
}

/// The retired `[packages]` section (cli.md §6.7).
///
/// Kept **readable only**, so the one-shot migration can find an old root, import it into
/// the catalog, and drop the key. Nothing writes it any more: where a package lives is an
/// observation the catalog records, not a setting to maintain, and a configured root that
/// had to be re-walked on every maintain command is exactly what the catalog replaced.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PackagesConfig {
    /// The former local-packages root. `Some` only until the migration has run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<PathBuf>,
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

/// The **vaire home**: `VAIRE_HOME` if set and non-empty, else `~/.vaire`.
///
/// Distinct from the config home on purpose. `config.toml`/`credentials.toml` are things
/// a person writes and might sync between machines; the home holds state the tool
/// maintains about *this* machine — starting with the catalog, and later the store. A
/// plain `~/.vaire` rather than a platform-specific location because it is a working
/// directory a user is expected to be able to find, delete, and watch grow.
///
/// Falls back to `.vaire-home` in the cwd only when there is no home directory at all.
pub fn vaire_home() -> PathBuf {
    if let Ok(dir) = std::env::var("VAIRE_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    directories::UserDirs::new()
        .map(|d| d.home_dir().join(".vaire"))
        .unwrap_or_else(|| PathBuf::from(".vaire-home"))
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
            // Report the location only — never the error's rendered snippet. `toml`
            // echoes the offending source line, and in this file that line is a secret:
            // an unterminated string on the key line printed the whole API key to stderr,
            // which routinely lands in CI logs.
            let where_ = match e.span() {
                Some(span) => format!(" at byte {}", span.start),
                None => String::new(),
            };
            eprintln!(
                "warning: {}{}: malformed TOML (ignoring credentials file)",
                path.display(),
                where_
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

/// The `credentials.toml` key a registry's bearer token is stored under
/// (registry-server.md §2.3). One key per registry name, not one shared key, so logging
/// in to a second registry never overwrites the first.
fn registry_token_key(registry: &str) -> String {
    format!("registry_{registry}_token")
}

/// The environment variable a per-registry token override reads — `VAIRE_TOKEN_<NAME>`,
/// uppercased and with anything that is not an ASCII letter/digit turned into `_`, so a
/// registry name with a `.` or `-` in it still yields a shell-legal variable name.
fn registry_token_env(registry: &str) -> String {
    let upper: String = registry
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("VAIRE_TOKEN_{upper}")
}

/// The bearer token to send to `registry`, or `None` for an anonymous request.
///
/// Precedence (registry-server.md §2.3, §4): a bare `VAIRE_TOKEN` — set in a CI job that
/// only ever talks to one registry — wins outright; then the per-registry
/// `VAIRE_TOKEN_<NAME>`, for a job juggling more than one; then whatever `vaire registry
/// login` stored in `credentials.toml`. A CI job therefore needs no login step at all.
pub fn registry_token(registry: &str) -> Option<String> {
    registry_token_from(registry, &config_home())
}

/// [`registry_token`] against an explicit config home (the DI seam for tests).
pub fn registry_token_from(registry: &str, home: &Path) -> Option<String> {
    if let Ok(v) = std::env::var("VAIRE_TOKEN")
        && !v.is_empty()
    {
        return Some(v);
    }
    credential_from(&registry_token_env(registry), home)
        .or_else(|| credential_from(&registry_token_key(registry), home))
}

/// Store the token `vaire registry login` obtained for `registry`.
pub fn save_registry_token(registry: &str, token: &str) -> Result<PathBuf> {
    save_credential(&registry_token_key(registry), token)
}

/// Forget the token for `registry` (`vaire registry logout`). `Ok(false)` when there was
/// none — logging out of a registry you never logged in to is not an error.
pub fn forget_registry_token(registry: &str) -> Result<bool> {
    forget_registry_token_from(registry, &config_home())
}

/// [`forget_registry_token`] against an explicit config home (the DI seam for tests).
pub fn forget_registry_token_from(registry: &str, home: &Path) -> Result<bool> {
    let path = home.join("credentials.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut table: toml::Table = toml::from_str(&text)
        .map_err(|e| VaireError::Config(format!("{}: {e}", path.display())))?;
    let key = registry_token_key(registry);
    if table.remove(&key).is_none() {
        return Ok(false);
    }
    let text = toml::to_string_pretty(&table)
        .map_err(|e| VaireError::Config(format!("serialize credentials: {e}")))?;
    write_private(&path, &text)?;
    Ok(true)
}

#[cfg(test)]
mod token_tests {
    use super::*;
    use std::sync::Mutex;

    /// `VAIRE_TOKEN` is process-global, and cargo runs a crate's tests on several
    /// threads by default — without this, a test that sets it can race a sibling test
    /// that assumes it is unset. Held for the duration of any test that touches the
    /// variable.
    static VAIRE_TOKEN_ENV: Mutex<()> = Mutex::new(());

    #[test]
    fn a_bare_vaire_token_wins_over_everything_per_registry() {
        let _guard = VAIRE_TOKEN_ENV.lock().unwrap();
        // SAFETY: no other thread reads or writes VAIRE_TOKEN while the guard above is
        // held — every test in this file that touches it takes the same lock first.
        unsafe { std::env::set_var("VAIRE_TOKEN", "global-token") };
        let dir = tempfile::tempdir().unwrap();
        save_credential_to(dir.path(), &registry_token_key("central"), "stored-token").unwrap();
        assert_eq!(
            registry_token_from("central", dir.path()),
            Some("global-token".to_string())
        );
        unsafe { std::env::remove_var("VAIRE_TOKEN") };
    }

    #[test]
    fn a_stored_login_is_used_when_nothing_in_the_environment_overrides_it() {
        let _guard = VAIRE_TOKEN_ENV.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(registry_token_from("central", dir.path()), None);
        save_registry_token_to(dir.path(), "central", "abc123").unwrap();
        assert_eq!(
            registry_token_from("central", dir.path()),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn logging_out_forgets_only_that_registrys_token() {
        let _guard = VAIRE_TOKEN_ENV.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        save_registry_token_to(dir.path(), "central", "abc123").unwrap();
        save_registry_token_to(dir.path(), "lab", "def456").unwrap();
        assert!(forget_registry_token_from("central", dir.path()).unwrap());
        assert_eq!(registry_token_from("central", dir.path()), None);
        assert_eq!(
            registry_token_from("lab", dir.path()),
            Some("def456".to_string())
        );
        // Logging out twice is not an error, just a no-op.
        assert!(!forget_registry_token_from("central", dir.path()).unwrap());
    }

    /// [`save_registry_token`] against an explicit home (the DI seam these tests need).
    fn save_registry_token_to(home: &Path, registry: &str, token: &str) -> Result<PathBuf> {
        save_credential_to(home, &registry_token_key(registry), token)
    }
}
