//! Writing `.vaire/packages/<name>` entries — the one place a link is created.
//!
//! Two callers share this: `vaire add --link <path>` (explicit — a bad path is a usage
//! error) and local-packages discovery (automatic — a failure is a reported note). The
//! validation is identical; only the error *phrasing* differs, so planning returns a
//! structured [`LinkError`] and the caller renders it.
//!
//! Planning is separated from committing so a rejection leaves **nothing** written: `vaire
//! add --link` validates before it touches the manifest (cli.md §4.2a).

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::corpus::repo::Repo;
use crate::error::Result;

/// Why a link could not be planned. The cases differ only in how a caller phrases them:
/// `Target` is about the package being linked to, `Entry` about the slot it goes in.
#[derive(Debug)]
pub enum LinkError {
    Target(String),
    Entry(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Target(m) | LinkError::Entry(m) => f.write_str(m),
        }
    }
}

/// A fully-validated link, ready to commit: everything that can be *rejected* has been.
pub struct LinkPlan {
    entry: PathBuf,
    /// The symlink target as it will be stored (relative to the packages dir when the
    /// paths share a prefix — portable if the checkout and target move together).
    pub stored: PathBuf,
}

/// What sits at a `.vaire/packages/<name>` entry right now.
#[derive(Debug, PartialEq, Eq)]
pub enum EntryState {
    /// Nothing there.
    Absent,
    /// A symlink whose target no longer exists — the one state discovery may replace.
    Broken,
    /// A resolvable symlink or a real directory. Never replaced automatically.
    Present,
}

pub fn entry_state(entry: &Path) -> EntryState {
    if entry.symlink_metadata().is_err() {
        return EntryState::Absent;
    }
    match std::fs::canonicalize(entry) {
        Ok(_) => EntryState::Present,
        Err(_) => EntryState::Broken,
    }
}

/// Validate and stage a link: canonicalize the target, require a package **declaring**
/// `name` (identity is declared, never path-derived), prepare `.vaire/packages/` (with the
/// derived-dir gitignore), and refuse a **real directory** at the entry — it may be
/// installed content. Nothing is written except the derived directories.
pub fn plan(root: &Path, name: &str, path: &Path) -> std::result::Result<LinkPlan, LinkError> {
    let target = std::fs::canonicalize(path).map_err(|e| LinkError::Target(e.to_string()))?;
    let target_manifest = target.join("knowledge.toml");
    if !target_manifest.is_file() {
        return Err(LinkError::Target(
            "not a package (no knowledge.toml)".to_string(),
        ));
    }
    let config = Config::load(&target_manifest).map_err(|e| LinkError::Target(e.to_string()))?;
    if config.name != name {
        return Err(LinkError::Target(format!(
            "that package declares name '{}', not '{name}' — identity is declared, never path-derived",
            config.name
        )));
    }

    let packages = Repo::prepare_packages_dir(root).map_err(|e| LinkError::Entry(e.to_string()))?;
    let derived = Repo::prepare_derived_dir(root).map_err(|e| LinkError::Entry(e.to_string()))?;
    Repo::ensure_derived_gitignore(&derived).map_err(|e| LinkError::Entry(e.to_string()))?;
    // Canonicalize so the relative computation sees the same prefix shape as the
    // (already canonical) target — e.g. macOS's /var → /private/var.
    let entry = packages.join(name);
    if let Ok(meta) = std::fs::symlink_metadata(&entry)
        && !meta.file_type().is_symlink()
    {
        return Err(LinkError::Entry(format!(
            "{} exists and is a real directory (not a link) — refusing to replace it",
            entry.display()
        )));
    }

    let stored = super::relative_to(&target, &packages).unwrap_or(target);
    Ok(LinkPlan { entry, stored })
}

/// Commit a staged link: create the symlink at a temporary name, then rename over the
/// entry — atomically replacing an existing symlink, so a reader never observes the entry
/// missing. Returns the stored target.
pub fn commit(plan: LinkPlan) -> Result<String> {
    let tmp = plan.entry.with_file_name(format!(
        ".{}.tmp-{}",
        plan.entry.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let _ = remove_symlink(&tmp);
    symlink_dir(&plan.stored, &tmp)?;
    // Windows cannot rename over an existing directory symlink; clear it first there.
    #[cfg(windows)]
    let _ = remove_symlink(&plan.entry);
    if let Err(e) = std::fs::rename(&tmp, &plan.entry) {
        let _ = remove_symlink(&tmp);
        return Err(e.into());
    }
    Ok(plan.stored.display().to_string())
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn remove_symlink(link: &Path) -> std::io::Result<()> {
    std::fs::remove_file(link)
}

/// Windows directory symlinks are directory entries — `remove_file` cannot delete them.
#[cfg(windows)]
fn remove_symlink(link: &Path) -> std::io::Result<()> {
    std::fs::remove_dir(link)
}
