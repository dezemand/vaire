//! File scanning — *where to look* (cli.md §6, design.md §9).
//!
//! The include/exclude globs only bound the search space; the typed `id:` in
//! frontmatter is still *what makes a file a node*. This module walks the working tree
//! under the include globs, skips the excludes, and yields candidate `.md` paths.
//! Parsing + discovery (does it actually have an `id:`?) happens downstream in
//! [`super::frontmatter`].

use std::path::PathBuf;

use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::error::{Result, VaireError};

/// Compiled include/exclude matchers built from [`Config`].
pub struct Scanner {
    include: GlobSet,
    exclude: GlobSet,
}

impl Scanner {
    pub fn from_config(config: &Config) -> Result<Scanner> {
        Ok(Scanner {
            include: build_globset(&config.include)?,
            exclude: build_globset(&config.exclude)?,
        })
    }

    /// Walk `repo` and return every candidate file (root-relative paths). Determinism:
    /// results are sorted so a rebuild over the same tree is byte-stable. `.git/` and
    /// `.vaire/` are never descended into — the corpus lives outside them.
    pub fn candidates(&self, repo: &Repo) -> Result<Vec<PathBuf>> {
        let root = repo.root();
        let mut out = Vec::new();
        let walker = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git") | Some(".vaire")));
        for entry in walker {
            let entry = entry.map_err(std::io::Error::from)?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
            if self.is_match(rel) {
                out.push(rel.to_path_buf());
            }
        }
        out.sort();
        Ok(out)
    }

    /// True iff `rel` (a repo-root-relative path) is in scope: matched by an include
    /// glob and not by any exclude glob.
    pub fn is_match(&self, rel: &std::path::Path) -> bool {
        self.include.is_match(rel) && !self.exclude.is_match(rel)
    }
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let glob = Glob::new(p).map_err(|e| VaireError::Config(format!("bad glob '{p}': {e}")))?;
        b.add(glob);
    }
    b.build()
        .map_err(|e| VaireError::Config(format!("glob set: {e}")))
}
