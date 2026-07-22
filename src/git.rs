//! Git — the provenance layer (design.md §10, "Git commit log is the provenance layer").
//!
//! Vairë shells out to the `git` binary rather than linking libgit2: it only needs a
//! handful of plumbing reads. Indexing is **bound to commit** (commit-as-publish) —
//! every index state corresponds to exactly one commit — so the index records which
//! commit it was built from and `status` compares that against HEAD.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

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
    require_commit_oid(since)?;
    let range = format!("{since}..HEAD");
    let out = run(repo_root, &["rev-list", "--count", &range])?;
    if !out.status.success() {
        return Ok(0);
    }
    Ok(stdout_trimmed(&out).parse().unwrap_or(0))
}

/// Files changed between `since` and HEAD — the input to an incremental reindex
/// (cli.md §4.1). Paths are repo-root-relative.
///
/// `None` means Git could not answer (most often `since` is no longer reachable after a
/// rebase/amend + gc, or a shallow clone). That is **not** the same as "nothing changed":
/// treating it as an empty diff would index nothing, then advance the commit anchor to
/// HEAD and strand every intervening edit in the index forever. The caller falls back to
/// a full rebuild instead.
pub fn changed_files(repo_root: &Path, since: &str) -> Result<Option<Vec<String>>> {
    require_commit_oid(since)?;
    let out = run(
        repo_root,
        &[
            "diff",
            "--name-only",
            "-z",
            "--end-of-options",
            since,
            "HEAD",
        ],
    )?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(nul_separated(&out)))
}

/// True for the full SHA-1/SHA-256 object IDs Git writes into index metadata.
pub fn is_commit_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn require_commit_oid(value: &str) -> Result<()> {
    if is_commit_oid(value) {
        Ok(())
    } else {
        Err(crate::error::VaireError::IndexCorrupt(format!(
            "invalid commit id in index metadata: {value:?}"
        )))
    }
}

/// Every file tracked at HEAD, repo-root-relative — the candidate set for a full build
/// over the *committed* tree (cli.md §4.1).
pub fn list_files_at_head(repo_root: &Path) -> Result<Vec<String>> {
    let out = run(repo_root, &["ls-tree", "-r", "--name-only", "-z", "HEAD"])?;
    Ok(nul_separated(&out))
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

/// Read many committed files through one long-lived Git process, preserving the supplied
/// path order. `git cat-file --batch` avoids paying process startup, repository discovery,
/// and pack setup once per Markdown file during a full index build.
pub fn show_many_at_head(repo_root: &Path, rel_paths: &[String]) -> Result<Vec<Option<String>>> {
    if rel_paths.is_empty() {
        return Ok(Vec::new());
    }

    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let input = rel_paths
        .iter()
        .map(|path| format!("HEAD:{path}\n"))
        .collect::<String>();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));

    // `cat-file` may emit diagnostics while it writes object data. Drain stderr in
    // parallel so a full error pipe cannot block stdout processing.
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut stderr = String::new();
        stderr_pipe.read_to_string(&mut stderr).map(|_| stderr)
    });

    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout);
    let result: Result<Vec<Option<String>>> = (|| {
        let mut out = Vec::with_capacity(rel_paths.len());
        for _ in rel_paths {
            let mut header = String::new();
            if reader.read_line(&mut header)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "git cat-file ended before returning every requested path",
                )
                .into());
            }
            let header = header.trim_end();
            if header.ends_with(" missing") {
                out.push(None);
                continue;
            }
            let size = header
                .split_whitespace()
                .last()
                .and_then(|part| part.parse::<usize>().ok())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unexpected git cat-file header: {header}"),
                    )
                })?;
            let mut bytes = vec![0; size];
            reader.read_exact(&mut bytes)?;
            let mut newline = [0; 1];
            reader.read_exact(&mut newline)?;
            if newline != *b"\n" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "git cat-file object was not newline terminated",
                )
                .into());
            }
            out.push(Some(String::from_utf8_lossy(&bytes).into_owned()));
        }
        Ok(out)
    })();

    if result.is_err() {
        let _ = child.kill();
    }
    // Always wait before returning so a parsing or I/O error cannot leave a Git child
    // behind. Keep the primary parsing error if there was one.
    let status = child.wait();
    let writer_result = writer
        .join()
        .map_err(|_| std::io::Error::other("git cat-file input writer panicked"))
        .and_then(|result| result);
    let stderr_result = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("git cat-file stderr reader panicked"))
        .and_then(|result| result);

    let out = result?;
    writer_result?;
    let stderr = stderr_result?;
    let status = status?;
    if !status.success() {
        return Err(
            std::io::Error::other(format!("git cat-file failed: {}", stderr.trim())).into(),
        );
    }
    Ok(out)
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

/// Split NUL-delimited `-z` output into paths.
///
/// Every path-listing call passes `-z` on purpose. Git's default `core.quotepath=true`
/// C-quotes any path with non-ASCII bytes on the newline-delimited forms — `café.md` comes
/// back as `"caf\303\251.md"`, which then fails the include globs *and* `cat-file`, so the
/// file is silently missing from the index. `-z` emits raw bytes and suppresses quoting.
fn nul_separated(out: &Output) -> Vec<String> {
    if !out.status.success() {
        return Vec::new();
    }
    out.stdout
        .split(|&b| b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::is_commit_oid;

    #[test]
    fn commit_oid_validation_accepts_only_full_hex_ids() {
        assert!(is_commit_oid(&"a".repeat(40)));
        assert!(is_commit_oid(&"B".repeat(64)));
        assert!(!is_commit_oid("--output=/tmp/file"));
        assert!(!is_commit_oid(&"a".repeat(39)));
        assert!(!is_commit_oid(&"z".repeat(40)));
    }
}
