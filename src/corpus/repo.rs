//! Repo discovery (cli.md §2.1).
//!
//! `vaire` operates on one corpus repository. It finds the root by walking up from the
//! working directory to the nearest directory containing `.git/`. `--repo` or
//! `VAIRE_REPO` override discovery (`--repo` wins). No repo found and none given ⇒
//! [`VaireError::NoRepo`] (exit `4`).

use std::path::{Path, PathBuf};

use crate::error::{Result, VaireError};

/// A located corpus repository and the conventional paths Vairë owns within it.
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Discover the repo root by the presence of a `.vaire/` directory (cli.md §2.1).
    ///
    /// Precedence: explicit `--repo` > `VAIRE_REPO` env > walk up from `start` to the
    /// nearest ancestor containing `.vaire/`. An explicit path (`--repo`/`VAIRE_REPO`)
    /// that lacks `.vaire/` is an error rather than a silent guess. The committed
    /// `.vaire/config.toml` is what marks a directory as a corpus.
    pub fn discover(explicit: Option<&Path>, start: &Path) -> Result<Repo> {
        if let Some(p) = explicit {
            return Self::require_vaire(p);
        }
        if let Ok(env) = std::env::var("VAIRE_REPO") {
            return Self::require_vaire(Path::new(&env));
        }
        let mut cur = Some(start);
        while let Some(dir) = cur {
            if dir.join(".vaire").is_dir() {
                return Ok(Repo {
                    root: dir.to_path_buf(),
                });
            }
            cur = dir.parent();
        }
        Err(VaireError::NoRepo)
    }

    fn require_vaire(p: &Path) -> Result<Repo> {
        if p.join(".vaire").is_dir() {
            Ok(Repo {
                root: p.to_path_buf(),
            })
        } else {
            Err(VaireError::NoRepo)
        }
    }

    /// Whether the corpus root is itself a Git repository (has its own `.git`). A corpus
    /// nested inside a larger repo is *not* one — it is indexed from the working tree.
    pub fn is_git_root(&self) -> bool {
        self.root.join(".git").exists()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.vaire/` — everything Vairë owns lives here (design.md §9).
    pub fn vaire_dir(&self) -> PathBuf {
        self.root.join(".vaire")
    }

    /// `<root>/.vaire/index.db` — the derived, gitignored index.
    pub fn index_db(&self) -> PathBuf {
        self.vaire_dir().join("index.db")
    }

    /// `<root>/.vaire/config.toml` — the one committed file under `.vaire/`.
    pub fn config_path(&self) -> PathBuf {
        self.vaire_dir().join("config.toml")
    }

    /// Make `abs` repo-root-relative and POSIX-slashed, the form returned by every
    /// read command (cli.md §2.4).
    pub fn relativize(&self, abs: &Path) -> String {
        let rel = abs.strip_prefix(&self.root).unwrap_or(abs);
        rel.to_string_lossy().replace('\\', "/")
    }
}
