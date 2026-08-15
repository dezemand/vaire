//! `vaire pack [--no-embeddings]` — build this package's distributable artifact
//! (registry.md §5). Maintain command — not on the MCP surface.
//!
//! The artifact is `<name>-<version>.tgz` in `.vaire/dist/`: a gzipped tar with a single
//! top-level directory holding the manifest, every corpus file the manifest's
//! include/exclude selects, **every file those reference** by relative link or image
//! (transitively through referenced Markdown — registry.md §5.2), and a freshly
//! exported `.vaire/index.db` (the machine-readable manifest — registry.md §5.1).
//! Everything is read **from the committed tree**: what you commit is what you publish,
//! and the artifact is reproducible because its inputs are a commit, not a mood.
//!
//! Pack is also a publication gate: it refuses to build when `vaire check` reports
//! violations, and it fails on a relative Markdown link whose target is missing from
//! the committed tree (registry.md §5.2). Links to targets the author chose to keep out
//! — exclude-glob-vetoed or gitignored — and a dirty working tree are warnings, not
//! stops. Orphans cannot exist: an unreferenced file simply does not ship.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::commands::Ctx;
use crate::config::Config;
use crate::corpus::markdown::{relative_link_targets, resolve_relative};
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

    // ---- the artifact file set: corpus, plus everything it references ------------------
    let head_files = git::list_files_at_head(root)?;
    let head_set: BTreeSet<&str> = head_files.iter().map(String::as_str).collect();
    let scanner = Scanner::from_config(&ctx.config)?;
    let selected = select_files(&head_files, &scanner);
    let payload = collect_referenced_files(
        root,
        "HEAD",
        Absent::AskGitignore,
        &selected,
        &head_set,
        &scanner,
        &mut warnings,
    )?;
    let mut selected = selected;
    selected.extend(payload);

    // ---- the artifact index (registry.md §5.1) -----------------------------------------
    let vaire_dir = Repo::prepare_derived_dir(root)?;
    let dist = vaire_dir.join("dist");
    std::fs::create_dir_all(&dist)?;
    let top = format!("{}-{}", ctx.config.name, ctx.config.version);
    let file_name = format!("{top}.tgz");
    // Scratch paths are pid-scoped (two packs of one package must not trample each
    // other's staging) and removed on every exit path, success or failure — a failed
    // pack never leaves a plausible-looking artifact or staging debris behind.
    let pid = std::process::id();
    let staging_db = dist.join(format!(".pack-index-{pid}.db"));
    let tmp = dist.join(format!(".{file_name}.{pid}.tmp"));
    let _scratch = RemoveOnDrop(vec![
        staging_db.clone(),
        std::path::PathBuf::from(format!("{}-wal", staging_db.display())),
        std::path::PathBuf::from(format!("{}-shm", staging_db.display())),
        tmp.clone(),
    ]);
    let stats = export::export_artifact_index(
        &ctx.repo.index_db(),
        &staging_db,
        !no_embeddings,
        concat!("vaire ", env!("CARGO_PKG_VERSION")),
    )?;

    // ---- the deterministic archive -----------------------------------------------------
    // Entry order is the BTreeMap's (sorted); timestamps are the commit's; ownership is
    // zeroed; the gzip header carries no timestamp. Same commit, same flags, same bytes.
    // Held fully in memory on purpose: knowledge corpora are megabytes, not gigabytes,
    // and streaming would buy nothing at this scale.
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

    let epoch = git::commit_epoch(root)?.unwrap_or(0).max(0) as u64;
    // Assemble beside the final name, promote by rename.
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

/// The artifact for a **released tag**, rebuilt from that tag's committed tree.
///
/// This is what makes `.vaire/dist/` a cache rather than a requirement (amendment 15):
/// `vaire push` works from a fresh clone because a tag carries everything the artifact is
/// made of. Three differences from [`run`], each of them a consequence of packing history
/// rather than the present:
///
/// * **No embedder.** The index is built by [`build::snapshot`], which reads a rev straight
///   into a scratch database with no vectors — and the publish default is stripped
///   artifacts anyway (§11), so there was nothing for an embedder to contribute. CI
///   therefore needs no embedding configuration to publish (amendment 21).
/// * **No `check` gate.** `vaire release` ran it before it created the tag. Re-running it
///   here would judge an old tree by today's rules, and a tag cannot be edited in response.
/// * **Nothing is written into the package.** The staging files live in `dest_dir`, so
///   packing a tag never touches the working checkout or its index.
/// * **A referenced file the tag does not carry is a warning, not a refusal**
///   ([`Absent::Warn`]). [`run`] separates "declared local-only" from "forgotten
///   `git add`" by asking `git check-ignore`, which only ever answers for the working
///   tree — so asking it here would make publishing a release depend on today's
///   `.gitignore`, and the tag cannot be edited in response to either answer.
pub fn at_rev(root: &Path, rev: &str, dest_dir: &Path) -> Result<RevArtifact> {
    let manifest = git::show_many_at(root, rev, &["knowledge.toml".to_string()])?
        .into_iter()
        .next()
        .flatten()
        .ok_or_else(|| {
            VaireError::Pack(format!(
                "{rev} has no knowledge.toml — it is not a package tree"
            ))
        })?;
    // Parsed from the tag, never from the working tree: the manifest is the artifact's
    // identity and its file-selection rules, and both are properties of the release.
    let config = Config::parse(&manifest, &format!("{rev}:knowledge.toml"))?;

    let files = git::list_files_at(root, rev)?;
    let file_set: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let scanner = Scanner::from_config(&config)?;
    let selected = select_files(&files, &scanner);
    let mut warnings = Vec::new();
    let payload = collect_referenced_files(
        root,
        rev,
        Absent::Warn,
        &selected,
        &file_set,
        &scanner,
        &mut warnings,
    )?;
    let mut selected = selected;
    selected.extend(payload);

    std::fs::create_dir_all(dest_dir)?;
    let top = format!("{}-{}", config.name, config.version);
    let file_name = format!("{top}.tgz");
    let pid = std::process::id();
    let snapshot_db = dest_dir.join(format!(".push-snapshot-{pid}.db"));
    let staging_db = dest_dir.join(format!(".push-index-{pid}.db"));
    let tmp = dest_dir.join(format!(".{file_name}.{pid}.tmp"));
    let _scratch = RemoveOnDrop(
        [&snapshot_db, &staging_db]
            .iter()
            .flat_map(|db| {
                ["", "-wal", "-shm"]
                    .map(|suffix| std::path::PathBuf::from(format!("{}{suffix}", db.display())))
            })
            .chain([tmp.clone()])
            .collect(),
    );

    build::snapshot(root, &config, rev, &snapshot_db)?;
    // Always stripped. A released artifact's checksum has to be reproducible for the
    // lockfile to mean anything, and vectors are consumer configuration (§11).
    export::export_artifact_index(
        &snapshot_db,
        &staging_db,
        false,
        concat!("vaire ", env!("CARGO_PKG_VERSION")),
    )?;

    let git_paths: Vec<String> = selected.iter().cloned().collect();
    let blobs = git::show_many_at_bytes(root, rev, &git_paths)?;
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (path, blob) in git_paths.iter().zip(blobs) {
        let bytes = blob.ok_or_else(|| {
            VaireError::Pack(format!(
                "{path} is listed at {rev} but has no readable blob"
            ))
        })?;
        entries.insert(path.clone(), bytes);
    }
    entries.insert(".vaire/index.db".to_string(), std::fs::read(&staging_db)?);

    let epoch = git::commit_epoch_at(root, rev)?.unwrap_or(0).max(0) as u64;
    write_archive(&tmp, &top, &entries, epoch)?;
    let artifact = dest_dir.join(&file_name);
    std::fs::rename(&tmp, &artifact)?;

    Ok(RevArtifact {
        name: config.name,
        version: config.version,
        description: config.description,
        dependencies: config.dependencies,
        sha256: crate::hash::sha256_file(&artifact)?,
        size_bytes: std::fs::metadata(&artifact)?.len(),
        entries: entries.len(),
        path: artifact,
        warnings,
    })
}

/// What to do about a referenced file the packed tree does not carry.
///
/// The distinction exists because `git check-ignore` only ever answers for the **working
/// tree**. Asking it about a tag would make publishing a release depend on today's
/// `.gitignore`: a rule added since would turn an old tag's broken link into a warning, or
/// its removal turn a declared-local-only target into a failure. Either way the tag is what
/// it is, and no answer to that question can be acted on.
#[derive(Clone, Copy)]
enum Absent {
    /// Packing the working tree: its ignore rules are the right thing to ask, and they
    /// separate "declared local-only" from "forgotten `git add`".
    AskGitignore,
    /// Packing history. Every absent target is reported and the artifact still builds —
    /// the alternative is refusing to publish a release nobody can edit in response, which
    /// would make the tool's own past permanently unpublishable.
    Warn,
}

/// An artifact rebuilt from a tag, plus what the wire needs to describe it.
///
/// `dependencies` rides along because the index document carries them (§8.3, decision 9):
/// transitive resolution must never download an artifact to read a manifest, so `push`
/// reads them here, from the manifest it already parsed.
pub struct RevArtifact {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub dependencies: std::collections::BTreeMap<String, String>,
    pub path: std::path::PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub entries: usize,
    pub warnings: Vec<String>,
}

/// The corpus files of a tree: the manifest, plus whatever the include/exclude globs select.
fn select_files(files: &[String], scanner: &Scanner) -> BTreeSet<String> {
    let mut selected: BTreeSet<String> = BTreeSet::new();
    selected.insert("knowledge.toml".to_string());
    for file in files {
        if scanner.is_match(Path::new(file)) {
            selected.insert(file.clone());
        }
    }
    selected
}

/// Grow the artifact's payload from references, validating every relative link.
///
/// **Inclusion is by reference, not location** (registry.md §5.2): every relative
/// `[]()`/`![]()` target reachable from the corpus files ships — transitively, when the
/// target is itself Markdown, so a shipped document never carries broken links of its
/// own. Three author decisions outrank a link, calibrated on real corpora: the
/// **exclude globs veto** shipment (warning — a stray link must not republish a draft);
/// a **gitignored** target is declared local-only (warning — togaf's raw `reference/`
/// corpus is the canonical case); anything else missing at HEAD (or escaping the
/// package root) **fails the pack** — a typo or a forgotten `git add`. A trailing-`/`
/// target is a directory link: satisfied by any shipped file under it, but never an
/// inclusion demand — a link asks for a file, not a tree.
///
/// `absent` decides only the middle of those: what to do about a target the tree does not
/// carry (see [`Absent`]).
fn collect_referenced_files(
    root: &Path,
    rev: &str,
    absent: Absent,
    corpus: &BTreeSet<String>,
    head_set: &BTreeSet<&str>,
    scanner: &Scanner,
    warnings: &mut Vec<String>,
) -> Result<BTreeSet<String>> {
    let mut payload: BTreeSet<String> = BTreeSet::new();
    let mut visited_md: BTreeSet<String> = corpus
        .iter()
        .filter(|p| p.ends_with(".md"))
        .cloned()
        .collect();
    let mut violations = Vec::new();
    // Targets absent from HEAD, held back for the gitignore tiebreaker below:
    // (source, line, target-as-written, path-to-ask-git-about, kind).
    let mut missing: Vec<(String, u32, String, String, &'static str)> = Vec::new();
    // Directory links validate against the *final* shipped set, so they are deferred:
    // (source, line, target-as-written, resolved-prefix).
    let mut dir_links: Vec<(String, u32, String, String)> = Vec::new();

    // Breadth-first over the reference graph, one Git batch per frontier.
    let mut frontier: Vec<String> = visited_md.iter().cloned().collect();
    while !frontier.is_empty() {
        let contents = git::show_many_at(root, rev, &frontier)?;
        let mut next = Vec::new();
        for (path, content) in frontier.iter().zip(contents) {
            let Some(content) = content else { continue };
            for (target, line) in relative_link_targets(&content) {
                let is_dir = target.ends_with('/');
                match resolve_relative(path, &target) {
                    None => violations.push(format!(
                        "{path}:{line} → {target} (escapes the package root)"
                    )),
                    Some(resolved) if is_dir => {
                        dir_links.push((
                            path.clone(),
                            line,
                            target.clone(),
                            format!("{resolved}/"),
                        ));
                    }
                    Some(resolved) => {
                        if corpus.contains(&resolved) || payload.contains(&resolved) {
                            // Already shipping.
                        } else if !head_set.contains(resolved.as_str()) {
                            missing.push((path.clone(), line, target.clone(), resolved, "file"));
                        } else if scanner.is_excluded(Path::new(&resolved)) {
                            warnings.push(format!(
                                "{path}:{line} links to {target}, which the exclude globs \
                                 veto; it stays out of the artifact"
                            ));
                        } else {
                            payload.insert(resolved.clone());
                            if resolved.ends_with(".md") && visited_md.insert(resolved.clone()) {
                                next.push(resolved);
                            }
                        }
                    }
                }
            }
        }
        frontier = next;
    }

    for (path, line, target, prefix) in dir_links {
        if corpus
            .iter()
            .chain(payload.iter())
            .any(|p| p.starts_with(&prefix))
        {
            continue; // at least one shipped file materializes the directory
        }
        if head_set.iter().any(|p| p.starts_with(&prefix)) {
            warnings.push(format!(
                "{path}:{line} links to {target}, but nothing under it ships in the artifact"
            ));
        } else {
            missing.push((path, line, target, prefix, "directory"));
        }
    }

    // The tiebreaker for a target absent from the tree: the repository's own ignore rules.
    // A gitignored target was *declared* local-only by the author — warn. An unignored one
    // is a typo or a forgotten `git add` — fail. Only available when the working tree is
    // the thing being packed; see [`Absent`].
    match (missing.is_empty(), absent) {
        (true, _) => {}
        (false, Absent::AskGitignore) => {
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
        (false, Absent::Warn) => {
            for (path, line, target, _, kind) in missing {
                warnings.push(format!(
                    "{path}:{line} links to {target}, and no such {kind} is committed at \
                     {rev}; it is not in the artifact"
                ));
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
    Ok(payload)
}

/// Best-effort scratch cleanup on every exit path — a `?` anywhere in `run` must not
/// leave staging files in `.vaire/dist/`. Missing files are fine (the success path has
/// already renamed or the failure happened before they existed).
struct RemoveOnDrop(Vec<std::path::PathBuf>);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
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
