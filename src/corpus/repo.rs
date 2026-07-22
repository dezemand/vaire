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
    /// Discover the package root by the presence of a committed `knowledge.toml` (v0.2).
    ///
    /// Precedence: explicit `--repo` > `VAIRE_REPO` env > walk up from `start` to the
    /// nearest ancestor containing `knowledge.toml`. An explicit path (`--repo`/`VAIRE_REPO`)
    /// that lacks `knowledge.toml` is an error rather than a silent guess. `knowledge.toml`
    /// is what marks a directory as a package; `.vaire/` holds only the derived index now.
    pub fn discover(explicit: Option<&Path>, start: &Path) -> Result<Repo> {
        if let Some(p) = explicit {
            return Self::require_manifest(p);
        }
        if let Ok(env) = std::env::var("VAIRE_REPO") {
            return Self::require_manifest(Path::new(&env));
        }
        let mut cur = Some(start);
        // Remember the nearest legacy `.vaire/config.toml` seen; if the walk finds no
        // `knowledge.toml`, that's a not-yet-migrated corpus — point the user at `vaire init`.
        let mut legacy: Option<PathBuf> = None;
        while let Some(dir) = cur {
            if dir.join("knowledge.toml").is_file() {
                return Ok(Repo {
                    root: dir.to_path_buf(),
                });
            }
            if legacy.is_none() && dir.join(".vaire").join("config.toml").is_file() {
                legacy = Some(dir.to_path_buf());
            }
            cur = dir.parent();
        }
        match legacy {
            Some(dir) => Err(VaireError::LegacyConfig(dir.display().to_string())),
            None => Err(VaireError::NoRepo),
        }
    }

    fn require_manifest(p: &Path) -> Result<Repo> {
        if p.join("knowledge.toml").is_file() {
            Ok(Repo {
                root: p.to_path_buf(),
            })
        } else if p.join(".vaire").join("config.toml").is_file() {
            Err(VaireError::LegacyConfig(p.display().to_string()))
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

    /// Create and return the package-owned derived directory. The directory itself must not
    /// be a symlink: it contains mutable state, and following a package-controlled symlink
    /// here would let an index build write outside the package root.
    pub fn prepare_derived_dir(root: &Path) -> Result<PathBuf> {
        let canonical_root = std::fs::canonicalize(root)?;
        let dir = root.join(".vaire");
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(VaireError::IndexCorrupt(format!(
                    "refusing symlinked derived directory: {}",
                    dir.display()
                )));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(VaireError::IndexCorrupt(format!(
                    "derived path is not a directory: {}",
                    dir.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&dir)?,
            Err(e) => return Err(e.into()),
        }
        let canonical_dir = std::fs::canonicalize(&dir)?;
        if canonical_dir.parent() != Some(canonical_root.as_path()) {
            return Err(VaireError::IndexCorrupt(format!(
                "derived directory escapes package root: {}",
                dir.display()
            )));
        }
        Ok(canonical_dir)
    }

    /// Create and return the package-owned dependency-link directory. Like `.vaire`, this
    /// is mutable state and must not be a symlink to a location outside the package.
    pub fn prepare_packages_dir(root: &Path) -> Result<PathBuf> {
        let vaire_dir = Self::prepare_derived_dir(root)?;
        let packages = vaire_dir.join("packages");
        match std::fs::symlink_metadata(&packages) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(VaireError::IndexCorrupt(format!(
                    "refusing symlinked packages directory: {}",
                    packages.display()
                )));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(VaireError::IndexCorrupt(format!(
                    "packages path is not a directory: {}",
                    packages.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&packages)?,
            Err(e) => return Err(e.into()),
        }
        let canonical_packages = std::fs::canonicalize(&packages)?;
        if canonical_packages.parent() != Some(vaire_dir.as_path()) {
            return Err(VaireError::IndexCorrupt(format!(
                "packages directory escapes derived directory: {}",
                packages.display()
            )));
        }
        Ok(canonical_packages)
    }

    /// Resolve an index-provided relative path safely beneath `root`. Index files are
    /// rebuildable caches, so their paths are untrusted input when opening a package.
    pub fn safe_file_under(root: &Path, relative: &str) -> Result<PathBuf> {
        use std::path::Component;

        let path = Path::new(relative);
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(VaireError::IndexCorrupt(format!(
                "index path escapes package root: {relative}"
            )));
        }
        let canonical_root = std::fs::canonicalize(root)?;
        let candidate = std::fs::canonicalize(root.join(path))?;
        if !candidate.starts_with(&canonical_root) || !candidate.is_file() {
            return Err(VaireError::IndexCorrupt(format!(
                "index path is not a file within the package: {relative}"
            )));
        }
        Ok(candidate)
    }

    /// `<root>/.vaire/index.db` — the derived, gitignored index.
    pub fn index_db(&self) -> PathBuf {
        self.vaire_dir().join("index.db")
    }

    /// The index path for an arbitrary package root (linked dependencies open their own
    /// index at the same well-known location — design.md §9, federated index).
    pub fn index_db_at(root: &Path) -> PathBuf {
        root.join(".vaire").join("index.db")
    }

    /// `<root>/.vaire/packages/` — where this package's dependency links live
    /// (cli.md §6.5).
    pub fn packages_dir_at(root: &Path) -> PathBuf {
        root.join(".vaire").join("packages")
    }

    /// Write the derived dir's self-contained `.gitignore` if absent (design.md §9). A
    /// `.vaire/` can come into existence outside `vaire init` — an index build in a
    /// never-initialized package (notably a linked dependency during a consumer's ensure
    /// pass) or a first `vaire add --link` — and derived files must never show up as
    /// untracked noise in that package's repo.
    pub fn ensure_derived_gitignore(vaire_dir: &Path) -> std::io::Result<()> {
        let gitignore = vaire_dir.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(
                &gitignore,
                "# Vairë — derived index, rebuildable from the corpus files.\n*\n!.gitignore\n",
            )?;
        }
        Ok(())
    }

    /// `<root>/knowledge.toml` — the committed package manifest (v0.2).
    pub fn config_path(&self) -> PathBuf {
        self.root.join("knowledge.toml")
    }

    /// Make `abs` repo-root-relative and POSIX-slashed, the form returned by every
    /// read command (cli.md §2.4).
    pub fn relativize(&self, abs: &Path) -> String {
        let rel = abs.strip_prefix(&self.root).unwrap_or(abs);
        rel.to_string_lossy().replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::Repo;

    #[test]
    fn safe_file_under_rejects_absolute_and_parent_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("inside.md"), "ok").unwrap();

        assert!(Repo::safe_file_under(dir.path(), "inside.md").is_ok());
        assert!(Repo::safe_file_under(dir.path(), "../outside.md").is_err());
        assert!(Repo::safe_file_under(dir.path(), "/etc/passwd").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn derived_dir_rejects_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(target.path(), dir.path().join(".vaire")).unwrap();

        assert!(Repo::prepare_derived_dir(dir.path()).is_err());
    }
}
