//! Git — the provenance layer (design.md §10, "Git commit log is the provenance layer").
//!
//! Vairë shells out to the `git` binary rather than linking libgit2: it only needs a
//! handful of plumbing reads. Indexing is **bound to commit** (commit-as-publish) —
//! every index state corresponds to exactly one commit — so the index records which
//! commit it was built from and `status` compares that against HEAD.

use std::path::Path;
use std::process::{Command, Output};

use crate::error::Result;

/// The current HEAD commit (full SHA), or `None` if the repo has no commits yet.
pub fn head(repo_root: &Path) -> Result<Option<String>> {
    let out = run(repo_root, &["rev-parse", "HEAD"])?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(stdout_trimmed(&out)))
}

/// How many commits HEAD is ahead of `since` (the last-indexed commit). Drives
/// `status`'s `commits_behind_head` (cli.md §4.3). Best-effort: an unknown `since`
/// (e.g. history rewritten) reports `0` rather than erroring.
pub fn commits_ahead(repo_root: &Path, since: &str) -> Result<u32> {
    let range = format!("{since}..HEAD");
    let out = run(repo_root, &["rev-list", "--count", &range])?;
    if !out.status.success() {
        return Ok(0);
    }
    Ok(stdout_trimmed(&out).parse().unwrap_or(0))
}

/// Files changed between `since` and HEAD — the input to an incremental reindex
/// (cli.md §4.1). Paths are repo-root-relative.
pub fn changed_files(repo_root: &Path, since: &str) -> Result<Vec<String>> {
    let out = run(repo_root, &["diff", "--name-only", since, "HEAD"])?;
    Ok(lines(&out))
}

/// Every file tracked at HEAD, repo-root-relative — the candidate set for a full build
/// over the *committed* tree (cli.md §4.1).
pub fn list_files_at_head(repo_root: &Path) -> Result<Vec<String>> {
    let out = run(repo_root, &["ls-tree", "-r", "--name-only", "HEAD"])?;
    Ok(lines(&out))
}

/// Read the committed contents of `rel_path` at HEAD. `vaire index` indexes the
/// **committed** tree, not the dirty working tree (cli.md §4.1). `None` if the path is
/// not present at HEAD (uncommitted ⇒ scratch, deleted ⇒ drop from index).
pub fn show_at_head(repo_root: &Path, rel_path: &str) -> Result<Option<String>> {
    let spec = format!("HEAD:{rel_path}");
    let out = run(repo_root, &["show", &spec])?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

fn run(repo_root: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()?)
}

fn stdout_trimmed(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn lines(out: &Output) -> Vec<String> {
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}
