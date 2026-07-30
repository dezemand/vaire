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
///
/// A failure here is an error, never an empty listing. Both callers read "no files at
/// HEAD" as a fact about the corpus: `committed_matching` would index nothing and report
/// success, and `partition_changed` — which classifies a changed path as *deleted* when it
/// is absent from this set — would drop every changed file from the index instead of
/// reindexing it.
pub fn list_files_at_head(repo_root: &Path) -> Result<Vec<String>> {
    let out = run(repo_root, &["ls-tree", "-r", "--name-only", "-z", "HEAD"])?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "git ls-tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
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
/// path order. Lossy-UTF-8 view of [`show_many_at_head_bytes`] — the corpus reader wants
/// text; artifact packing (`vaire pack`) reads the byte form so attachments survive intact.
pub fn show_many_at_head(repo_root: &Path, rel_paths: &[String]) -> Result<Vec<Option<String>>> {
    Ok(show_many_at_head_bytes(repo_root, rel_paths)?
        .into_iter()
        .map(|blob| blob.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
        .collect())
}

/// Read many committed files as raw bytes through one long-lived Git process, preserving
/// the supplied path order. `git cat-file --batch` avoids paying process startup,
/// repository discovery, and pack setup once per file.
pub fn show_many_at_head_bytes(
    repo_root: &Path,
    rel_paths: &[String],
) -> Result<Vec<Option<Vec<u8>>>> {
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
    let result: Result<Vec<Option<Vec<u8>>>> = (|| {
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
            out.push(Some(bytes));
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

/// Which of `paths` the repository's ignore rules match — the author's own declaration
/// that something is local-only. Drives `vaire pack`'s missing-link tiebreaker: a
/// gitignored target was *chosen* to stay undistributed (warn), an unignored one is a
/// typo or a forgotten file (fail). Paths may be hypothetical; `git check-ignore`
/// evaluates patterns, not the filesystem. A git failure reports none ignored — the
/// strict (failing) direction.
pub fn ignored_paths(
    repo_root: &Path,
    paths: &[String],
) -> Result<std::collections::BTreeSet<String>> {
    if paths.is_empty() {
        return Ok(Default::default());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let input: Vec<u8> = paths
        .iter()
        .flat_map(|p| p.bytes().chain(std::iter::once(0)))
        .collect();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_end(&mut stdout)?;
    let status = child.wait()?;
    let _ = writer.join();
    // Exit 0 = some ignored, 1 = none; anything else means git could not answer.
    if !matches!(status.code(), Some(0) | Some(1)) {
        return Ok(Default::default());
    }
    Ok(stdout
        .split(|&b| b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect())
}

/// The committer timestamp of HEAD as a Unix epoch, or `None` without commits. `vaire
/// pack` pins artifact entry mtimes to this, so the same commit always produces the same
/// bytes (registry.md §5) while extracted files still carry a meaningful date.
pub fn commit_epoch(repo_root: &Path) -> Result<Option<i64>> {
    let out = run(repo_root, &["show", "-s", "--format=%ct", "HEAD"])?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(stdout_trimmed(&out).parse().ok())
}

/// Whether the working tree differs from HEAD (advisory — drives `vaire pack`'s
/// "you are packing the committed tree" warning, never a hard gate). `.vaire/` is
/// excluded: its self-contained `.gitignore` is derived state that may legitimately be
/// untracked. A failed `git status` reports clean — this is a warning source, not truth.
pub fn working_tree_dirty(repo_root: &Path) -> Result<bool> {
    let out = run(
        repo_root,
        &["status", "--porcelain", "--", ":(exclude).vaire"],
    )?;
    if !out.status.success() {
        return Ok(false);
    }
    Ok(!out.stdout.iter().all(|b| b.is_ascii_whitespace()))
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
///
/// Purely a parser: it does **not** inspect the exit status. Folding a failure in here as
/// an empty list is what let a failed `git` command masquerade as a legitimately empty
/// result — each caller must decide what a failure means for it, above.
fn nul_separated(out: &Output) -> Vec<String> {
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

    #[test]
    fn a_failed_ls_tree_is_an_error_not_an_empty_tree() {
        // Not a Git repository at all, so `git ls-tree HEAD` exits non-zero. Reporting an
        // empty listing here is unsafe in both callers: a full build would index nothing
        // and claim success, and `partition_changed` treats a changed path missing from
        // this set as *deleted*, so it would drop every changed file from the index.
        let dir = std::env::temp_dir().join(format!("vaire-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = super::list_files_at_head(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "expected an error, got {:?}",
            result.map(|v| v.len())
        );
    }
}
