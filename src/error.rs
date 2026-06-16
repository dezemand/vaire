//! Errors and their mapping to documented exit codes (cli.md §7).
//!
//! Every fallible path in the crate returns [`VaireError`]. The binary maps it to
//! one of the seven exit codes, and `--json` renders it as the single
//! `{"error": {...}}` shape so a machine consumer parses one thing unconditionally.

use serde::Serialize;

pub type Result<T> = std::result::Result<T, VaireError>;

/// The documented process exit codes (cli.md §7). The discriminant *is* the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// `0` — success. For `check`: no violations.
    Success = 0,
    /// `1` — generic/unexpected error.
    Generic = 1,
    /// `2` — usage error: bad flags or arguments.
    Usage = 2,
    /// `3` — index unreadable/corrupt; rebuild with `vaire index --full`.
    IndexCorrupt = 3,
    /// `4` — no corpus repo found, or index not built yet (read commands).
    NoRepoOrIndex = 4,
    /// `5` — ID not found (`resolve`, `backlinks`, `refs`).
    IdNotFound = 5,
    /// `6` — `vaire check` found violations (or warnings under `--strict`).
    CheckViolations = 6,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// A stable, machine-readable error kind. Serialized into the JSON error shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Generic,
    Usage,
    IndexCorrupt,
    NoRepo,
    IndexNotBuilt,
    IdNotFound,
    CheckViolations,
}

#[derive(Debug, thiserror::Error)]
pub enum VaireError {
    #[error("usage error: {0}")]
    Usage(String),

    #[error(
        "no corpus found: no .vaire/ directory here or in any parent (point --repo at a corpus root, or create .vaire/config.toml to mark one)"
    )]
    NoRepo,

    #[error("index not built yet at {0}; run `vaire index`")]
    IndexNotBuilt(String),

    #[error("index is corrupt and cannot be opened ({0}); rebuild with `vaire index --full`")]
    IndexCorrupt(String),

    #[error("no node with id '{0}'")]
    IdNotFound(String),

    #[error("`vaire check` found {0} violation(s)")]
    CheckViolations(usize),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error(transparent)]
    Other(#[from] anyhow_like::Boxed),
}

impl VaireError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            VaireError::Usage(_) => ExitCode::Usage,
            VaireError::NoRepo => ExitCode::NoRepoOrIndex,
            VaireError::IndexNotBuilt(_) => ExitCode::NoRepoOrIndex,
            VaireError::IndexCorrupt(_) => ExitCode::IndexCorrupt,
            VaireError::IdNotFound(_) => ExitCode::IdNotFound,
            VaireError::CheckViolations(_) => ExitCode::CheckViolations,
            _ => ExitCode::Generic,
        }
    }

    pub fn kind(&self) -> ErrorKind {
        match self {
            VaireError::Usage(_) => ErrorKind::Usage,
            VaireError::NoRepo => ErrorKind::NoRepo,
            VaireError::IndexNotBuilt(_) => ErrorKind::IndexNotBuilt,
            VaireError::IndexCorrupt(_) => ErrorKind::IndexCorrupt,
            VaireError::IdNotFound(_) => ErrorKind::IdNotFound,
            VaireError::CheckViolations(_) => ErrorKind::CheckViolations,
            _ => ErrorKind::Generic,
        }
    }

    /// The `{"error": {...}}` payload emitted on stdout under `--json` (cli.md §7).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": {
                "code": self.exit_code().code(),
                "kind": self.kind(),
                "message": self.to_string(),
            }
        })
    }
}

/// A tiny boxed-error shim so we can `?`-convert arbitrary errors without pulling in
/// `anyhow`. Anything that is `std::error::Error` collapses into [`VaireError::Other`].
pub mod anyhow_like {
    #[derive(Debug, thiserror::Error)]
    #[error(transparent)]
    pub struct Boxed(#[from] pub Box<dyn std::error::Error + Send + Sync>);
}
