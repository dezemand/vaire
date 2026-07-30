//! `vaire pack [--no-embeddings]` — build this package's distributable artifact
//! (registry.md §5). Maintain command — not on the MCP surface.
//!
//! The artifact is `<name>-<version>.tgz` in `.vaire/dist/`: a gzipped tar with a single
//! top-level directory holding the manifest, every corpus file the manifest's
//! include/exclude selects, `attachments/**`, and a freshly exported `.vaire/index.db`
//! (the machine-readable manifest — registry.md §5.1). Everything is read **from the
//! committed tree**: what you commit is what you publish, and the artifact is
//! reproducible because its inputs are a commit, not a mood.
//!
//! Pack is also a publication gate: it refuses to build when `vaire check` reports
//! violations, and it fails on a relative Markdown link whose target is missing from the
//! artifact (registry.md §5.2). Orphaned attachments and a dirty working tree are
//! warnings, not stops.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::commands::Ctx;
use crate::corpus::repo::Repo;
use crate::corpus::scan::Scanner;
use crate::error::{Result, VaireError};
use crate::git;
use crate::index::build::{self, Mode};
use crate::index::export;
use crate::output::PackOutput;

pub fn run(ctx: &Ctx, no_embeddings: bool) -> Result<PackOutput> {
    let root = ctx.repo.root();

    // ---- committed-tree preconditions -------------------------------------------------
    // Packing is commit-as-publish taken literally: no commit, no artifact. A corpus
    // nested inside a larger repo indexes from the working tree, but it cannot honestly
    // claim "this artifact is commit X", so it cannot pack.
    if !ctx.repo.is_git_root() {
        return Err(VaireError::Pack(format!(
            "`vaire pack` packs the committed tree, and {} is not a Git repository \
             (a corpus nested inside a larger repo cannot pack); run `git init` and commit first",
            root.display()
        )));
    }
    let head = git::head(root)?.ok_or_else(|| {
        VaireError::Pack("this repository has no commits yet; commit the corpus first".into())
    })?;
    let manifest_at_head = git::show_at_head(root, "knowledge.toml")?.ok_or_else(|| {
        VaireError::Pack("knowledge.toml is not committed; commit the manifest first".into())
    })?;
    let manifest_working = std::fs::read_to_string(root.join("knowledge.toml"))?;
    // The manifest is the artifact's identity and its file-selection rules; packing HEAD
    // with a diverged working manifest would build an artifact that matches neither.
    if manifest_at_head != manifest_working {
        return Err(VaireError::Pack(
            "knowledge.toml differs between HEAD and the working tree; \
             commit the manifest before packing"
                .into(),
        ));
    }

    let mut warnings = Vec::new();
    if git::working_tree_dirty(root)? {
        warnings.push(format!(
            "working tree has uncommitted changes; the artifact is the committed tree at {}",
            &head[..head.len().min(12)]
        ));
    }

    // ---- freshness + the publication gate ---------------------------------------------
    // Bring the index to HEAD, then hold the artifact to `vaire check`'s bar: an artifact
    // with violations would ship broken references to every consumer.
    let embedder = ctx.embedder()?;
    build::run(&ctx.repo, &ctx.config, embedder, Mode::Incremental)?;
    let (report, failed) = crate::commands::check::run(ctx, false, false, false)?;
    if failed {
        return Err(VaireError::CheckViolations(report.violations.len()));
    }

    // ---- the artifact file set ---------------------------------------------------------
    let head_files = git::list_files_at_head(root)?;
    let head_set: BTreeSet<&str> = head_files.iter().map(String::as_str).collect();
    let scanner = Scanner::from_config(&ctx.config)?;
    let mut selected: BTreeSet<String> = BTreeSet::new();
    selected.insert("knowledge.toml".to_string());
    for file in &head_files {
        if scanner.is_match(Path::new(file)) || file.starts_with("attachments/") {
            selected.insert(file.clone());
        }
    }

    // ---- relative-link integrity (registry.md §5.2) ------------------------------------
    let referenced_attachments = lint_relative_links(root, &selected, &head_set, &mut warnings)?;
    for orphan in selected
        .iter()
        .filter(|p| p.starts_with("attachments/") && !referenced_attachments.contains(*p))
    {
        warnings.push(format!("{orphan} is not referenced by any packed file"));
    }

    // ---- the artifact index (registry.md §5.1) -----------------------------------------
    let vaire_dir = Repo::prepare_derived_dir(root)?;
    let dist = vaire_dir.join("dist");
    std::fs::create_dir_all(&dist)?;
    let staging_db = dist.join(format!(".pack-index-{}.db", std::process::id()));
    let stats = export::export_artifact_index(
        &ctx.repo.index_db(),
        &staging_db,
        !no_embeddings,
        concat!("vaire ", env!("CARGO_PKG_VERSION")),
    )?;

    // ---- the deterministic archive -----------------------------------------------------
    // Entry order is the BTreeMap's (sorted); timestamps are the commit's; ownership is
    // zeroed; the gzip header carries no timestamp. Same commit, same flags, same bytes.
    let git_paths: Vec<String> = selected.iter().cloned().collect();
    let blobs = git::show_many_at_head_bytes(root, &git_paths)?;
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (path, blob) in git_paths.iter().zip(blobs) {
        let bytes = blob.ok_or_else(|| {
            VaireError::Pack(format!("{path} is listed at HEAD but has no readable blob"))
        })?;
        entries.insert(path.clone(), bytes);
    }
    entries.insert(".vaire/index.db".to_string(), std::fs::read(&staging_db)?);
    let _ = std::fs::remove_file(&staging_db);

    let epoch = git::commit_epoch(root)?.unwrap_or(0).max(0) as u64;
    let top = format!("{}-{}", ctx.config.name, ctx.config.version);
    let file_name = format!("{top}.tgz");
    // Assemble beside the final name, promote by rename — a failed pack never leaves a
    // plausible-looking artifact behind.
    let tmp = dist.join(format!(".{file_name}.tmp"));
    write_archive(&tmp, &top, &entries, epoch)?;
    let artifact = dist.join(&file_name);
    std::fs::rename(&tmp, &artifact)?;

    Ok(PackOutput {
        name: ctx.config.name.clone(),
        version: ctx.config.version.clone(),
        commit: head,
        // Stated, not derived: `dist` is canonicalized (prepare_derived_dir), and
        // relativizing a canonical path against a non-canonical root would leak an
        // absolute path (macOS: /var vs /private/var).
        artifact: format!(".vaire/dist/{file_name}"),
        sha256: crate::hash::sha256_file(&artifact)?,
        size_bytes: std::fs::metadata(&artifact)?.len(),
        entries: entries.len(),
        nodes: stats.nodes,
        embeddings: stats.embeddings,
        warnings,
    })
}

/// Check every relative Markdown link in the packed `.md` files against the artifact.
///
/// Two tiers, calibrated on real corpora: a target **missing at HEAD and not gitignored**
/// (or escaping the package root) fails the pack — that is the corruption class, a typo
/// or a forgotten `git add`. A target the author *chose* to keep out — excluded from the
/// artifact by include/exclude, or gitignored entirely — only warns: pointing at
/// deliberately-unshipped source material (a raw reference corpus, a draft) is
/// legitimate provenance. A trailing-`/` target is a directory link, satisfied by any
/// file under it.
///
/// Returns the set of `attachments/` paths that are referenced (for the orphan warning).
fn lint_relative_links(
    root: &Path,
    selected: &BTreeSet<String>,
    head_set: &BTreeSet<&str>,
    warnings: &mut Vec<String>,
) -> Result<BTreeSet<String>> {
    let md_paths: Vec<String> = selected
        .iter()
        .filter(|p| p.ends_with(".md"))
        .cloned()
        .collect();
    let contents = git::show_many_at_head(root, &md_paths)?;

    let mut referenced = BTreeSet::new();
    let mut violations = Vec::new();
    // Targets absent from HEAD, held back for the gitignore tiebreaker below:
    // (source, line, target-as-written, path-to-ask-git-about, kind).
    let mut missing: Vec<(String, u32, String, String, &'static str)> = Vec::new();
    for (path, content) in md_paths.iter().zip(contents) {
        let Some(content) = content else { continue };
        for (target, line) in relative_link_targets(&content) {
            let is_dir = target.ends_with('/');
            match resolve_relative(path, &target) {
                None => violations.push(format!(
                    "{path}:{line} → {target} (escapes the package root)"
                )),
                Some(resolved) if is_dir => {
                    let prefix = format!("{resolved}/");
                    if selected.iter().any(|p| p.starts_with(&prefix)) {
                        // At least one packed file materializes the directory.
                    } else if head_set.iter().any(|p| p.starts_with(&prefix)) {
                        warnings.push(format!(
                            "{path}:{line} links to {target}, which is excluded from the \
                             artifact by include/exclude"
                        ));
                    } else {
                        missing.push((path.clone(), line, target.clone(), prefix, "directory"));
                    }
                }
                Some(resolved) => {
                    if selected.contains(&resolved) {
                        if resolved.starts_with("attachments/") {
                            referenced.insert(resolved);
                        }
                    } else if head_set.contains(resolved.as_str()) {
                        warnings.push(format!(
                            "{path}:{line} links to {target}, which is excluded from the \
                             artifact by include/exclude"
                        ));
                    } else {
                        missing.push((path.clone(), line, target.clone(), resolved, "file"));
                    }
                }
            }
        }
    }

    // The tiebreaker for a target absent from HEAD: the repository's own ignore rules.
    // A gitignored target was *declared* local-only by the author (togaf's raw
    // `reference/` corpus is the canonical case) — warn. An unignored one is a typo or
    // a forgotten `git add` — fail.
    if !missing.is_empty() {
        let ask: Vec<String> = missing.iter().map(|(_, _, _, p, _)| p.clone()).collect();
        let ignored = git::ignored_paths(root, &ask)?;
        for (path, line, target, asked, kind) in missing {
            if ignored.contains(&asked) {
                warnings.push(format!(
                    "{path}:{line} links to {target}, which is gitignored (local-only by \
                     this package's own declaration) and not distributed"
                ));
            } else {
                violations.push(format!("{path}:{line} → {target} (no such {kind} at HEAD)"));
            }
        }
    }

    if !violations.is_empty() {
        return Err(VaireError::Pack(format!(
            "{} broken relative link(s) — an artifact must be self-contained:\n  {}",
            violations.len(),
            violations.join("\n  ")
        )));
    }
    Ok(referenced)
}

/// Extract relative link/image targets (`[text](target)`) with 1-based line numbers.
/// Deliberately narrow: inline links only (no reference-style definitions), fenced code
/// blocks skipped, and anything URL-shaped (`https://…`, `mailto:`, bare `#anchor`)
/// ignored — those are not files this artifact must carry. Wikilinks (`[[type:id]]`)
/// never match: they have no `](`, and `vaire check` owns them.
fn relative_link_targets(content: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, raw_line) in content.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut rest = raw_line;
        while let Some(idx) = rest.find("](") {
            let after = &rest[idx + 2..];
            let (raw_target, remainder) = match after.strip_prefix('<') {
                // `](<path with spaces>)` — the brackets exist to permit spaces, so no
                // title-splitting applies inside them.
                Some(bracketed) => match bracketed.split_once('>') {
                    Some((t, r)) => (t, r),
                    None => break,
                },
                None => match after.split_once(')') {
                    // `](path "title")` — the title is not part of the path.
                    Some((t, r)) => (t.split(char::is_whitespace).next().unwrap_or(t), r),
                    None => break,
                },
            };
            rest = remainder;
            if let Some(target) = classify(raw_target) {
                out.push((target, line_no));
            }
        }
    }
    out
}

/// Reduce a raw link target to a relative file path worth checking, or `None` for
/// targets that are not package files (URLs, anchors, empty). A leading-`/` "absolute"
/// path is returned as-is so resolution can flag it — corpus Markdown is portable and an
/// absolute path is broken everywhere but one machine.
fn classify(raw: &str) -> Option<String> {
    let mut target = raw.trim();
    // `path#fragment` — the file is what must exist.
    if let Some((path, _fragment)) = target.split_once('#') {
        target = path;
    }
    if target.is_empty() {
        return None;
    }
    // A scheme (`https://…`, `mailto:…`, `tel:…`) marks an external target: a colon
    // before any path separator. Relative file paths cannot contain one there.
    let head = target.split('/').next().unwrap_or(target);
    if head.contains(':') {
        return None;
    }
    Some(target.to_string())
}

/// Resolve `target` relative to `source` (both `/`-separated, package-root-relative).
/// `None` when the target escapes the package root (including absolute paths).
fn resolve_relative(source: &str, target: &str) -> Option<String> {
    if target.starts_with('/') {
        return None;
    }
    let mut stack: Vec<&str> = source.split('/').collect();
    stack.pop(); // the source file itself; links resolve from its directory
    for comp in target.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }
    Some(stack.join("/"))
}

/// Write the artifact archive: a gzip stream (no embedded timestamp, fixed level) over a
/// tar whose entries appear in `entries` (sorted) order with pinned metadata.
fn write_archive(
    path: &Path,
    top: &str,
    entries: &BTreeMap<String, Vec<u8>>,
    epoch: u64,
) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let gz = flate2::GzBuilder::new().mtime(0).write(
        file,
        // Pinned, not `default()`: the compression level is part of "same bytes".
        flate2::Compression::new(6),
    );
    let mut tar = tar::Builder::new(gz);
    for (rel, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(epoch);
        header.set_uid(0);
        header.set_gid(0);
        // `append_data` computes the checksum and handles long names (GNU extension).
        tar.append_data(&mut header, format!("{top}/{rel}"), bytes.as_slice())?;
    }
    let gz = tar.into_inner()?;
    let file = gz.finish()?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{classify, relative_link_targets, resolve_relative};

    #[test]
    fn extracts_relative_targets_with_lines() {
        let md = "# T\n\nSee [spec](docs/spec.md) and ![wiring](../attachments/w.png).\n\n\
                  ```\n[not a link](inside/fence.md)\n```\n\nAlso [ext](https://x.example) \
                  and [anchor](#top) and [mail](mailto:a@b.c).\n";
        let links = relative_link_targets(md);
        assert_eq!(
            links,
            vec![
                ("docs/spec.md".to_string(), 3),
                ("../attachments/w.png".to_string(), 3),
            ]
        );
    }

    #[test]
    fn titles_are_split_in_extraction() {
        let md = "[doc](a.md \"the title\")\n";
        assert_eq!(relative_link_targets(md), vec![("a.md".to_string(), 1)]);
    }

    #[test]
    fn classify_strips_fragments_and_schemes() {
        assert_eq!(classify("a.md#sec"), Some("a.md".into()));
        assert_eq!(classify("#only-anchor"), None);
        assert_eq!(classify("https://x.example/p"), None);
        assert_eq!(classify("tel:123"), None);
        assert_eq!(classify(""), None);
        // Absolute paths survive classification so resolution can reject them.
        assert_eq!(classify("/etc/passwd"), Some("/etc/passwd".into()));
    }

    #[test]
    fn angle_bracket_targets_keep_spaces() {
        let md = "[doc](<my file.md>)\n";
        assert_eq!(
            relative_link_targets(md),
            vec![("my file.md".to_string(), 1)]
        );
    }

    #[test]
    fn resolution_is_directory_relative_and_containment_checked() {
        assert_eq!(
            resolve_relative("knowledge/a/b.md", "../x.md"),
            Some("knowledge/x.md".into())
        );
        assert_eq!(
            resolve_relative("readme.md", "attachments/p.png"),
            Some("attachments/p.png".into())
        );
        assert_eq!(resolve_relative("a.md", "../../out.md"), None);
        assert_eq!(resolve_relative("a.md", "/abs.md"), None);
    }
}
