//! `vaire release [--major] [--dry-run] [--notes <file>] [--allow-branch]` (cli.md §4.7).
//! Maintain command — not on the MCP surface.
//!
//! Collapses the release ritual into one command whose version number is **computed, not
//! typed**: classify against the last release, gate on the integrity checks, write the
//! version into the manifest, write the release record, commit, tag.
//!
//! Two things it deliberately does not do. It never runs `git push` — git transport stays
//! the maintainer's, exactly as it was before this command existed — and it never uploads
//! anything: publishing to a registry is `vaire push`, a separate, idempotent, retryable
//! act, so a flaky upload re-runs an upload rather than a ritual, and CI can publish a tag
//! it did not cut.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, value};

use crate::commands::Ctx;
use crate::corpus::repo::Repo;
use crate::corpus::scan::Scanner;
use crate::error::{Result, VaireError};
use crate::index::Index;
use crate::index::build::{self, Mode};
use crate::model::{Bump, Version};
use crate::output::ReleaseOutput;
use crate::release::{classify, record};

/// Everything the CLI can ask of a release.
#[derive(Debug, Default, Clone, Copy)]
pub struct Options<'a> {
    /// Consent to a MAJOR: required when the classifier sees one, and sufficient to
    /// escalate to one it cannot see.
    pub major: bool,
    /// Classify and report; write nothing.
    pub dry_run: bool,
    /// Accept advisory prompts without asking — the CI posture.
    pub yes: bool,
    /// A file holding the invalidated-assumptions notes a MAJOR requires.
    pub notes: Option<&'a Path>,
    /// Release from a branch that is not the repository's mainline.
    pub allow_branch: bool,
}

pub fn run(ctx: &Ctx, options: Options<'_>) -> Result<ReleaseOutput> {
    let root = ctx.repo.root();
    let package = ctx.config.name.clone();
    preflight_git(ctx, &options)?;

    // What we are releasing *from*: the last tagged version if there is one, else what
    // the manifest currently claims. The tag wins on a disagreement — it records what was
    // actually published, while a manifest version can be edited by anyone.
    let baseline = crate::release::latest_release(root, &package)?;
    let current_version: Version = match &baseline {
        Some((_, version)) => *version,
        None => ctx.config.version.parse().map_err(|_| {
            VaireError::Release(format!(
                "knowledge.toml declares version {:?}, which is not MAJOR.MINOR.PATCH",
                ctx.config.version
            ))
        })?,
    };

    // Refresh the index to HEAD, then classify against the baseline. The live index is the
    // "after" side, so the same code that answers every other query decides the release.
    let embedder = ctx.embedder()?;
    build::run(&ctx.repo, &ctx.config, embedder, Mode::Incremental)?;
    let index = ctx.open_index()?;

    let scratch = Scratch::new(root, "release-baseline")?;
    let classification = match &baseline {
        None => classify::initial(),
        Some((tag, _)) => {
            build::snapshot(root, &baseline_config(ctx, tag), tag, &scratch.db)?;
            let before = Index::open(&scratch.db)?;
            classify::diff(&before, &index, &ctx.config.release_type)?
        }
    };
    let heavily_cited = heavily_cited(&index, &classification)?;
    drop(index);
    drop(scratch);

    // Nothing changed since the last release: a clean no-op, not a failure. An automated
    // pipeline runs this on every merge, and most merges do not warrant a version.
    if matches!(classification.outcome, classify::Outcome::Nothing) {
        return Ok(ReleaseOutput::nothing(
            package,
            current_version,
            classification,
        ));
    }

    // `--major` may always escalate: a truth reversal can be one word wide, and the
    // classifier only sees structure.
    let computed = classification.bump();
    let bump = match (computed, options.major) {
        (_, true) => Some(Bump::Major),
        (Some(Bump::Major), false) => {
            // Not an error — a decision the tool cannot make. The caller turns this into
            // its own exit code so a pipeline reports "needs a human", not "broke".
            return Ok(ReleaseOutput::blocked(
                package,
                current_version,
                classification,
            ));
        }
        (bump, false) => bump,
    };
    let version = match bump {
        Some(bump) => current_version.bumped(bump),
        // A first release publishes what the manifest already declares: there is nothing
        // to diff, so there is nothing to increment.
        None => current_version,
    };

    let notes = read_notes(options.notes)?;
    if bump == Some(Bump::Major) && notes.is_none() {
        return Err(VaireError::Release(format!(
            "a major release must say what it invalidates — write the notes and pass \
             `--notes <file>`.\n  \
             Dependents read that text to decide whether their references still hold \
             ({} entities removed, {} retired)",
            classification.removed.len(),
            classification.retired.len()
        )));
    }

    let tag = crate::release::tag_name(&package, version, false);
    let record_path = record::path_for(&ctx.config, version);
    if options.dry_run {
        return Ok(ReleaseOutput::planned(
            package,
            version,
            bump,
            classification,
            tag,
            record_path.to_string_lossy().replace('\\', "/"),
        )
        .with_advisories(heavily_cited));
    }
    preflight_writes(ctx, &tag, &record_path)?;
    confirm_patch(&heavily_cited, &options)?;

    // The integrity gate. Violations refuse the release for the same reason they refuse a
    // pack: a published version with dangling references ships them to every consumer.
    // Warnings are reported, never fatal — an inline-rich corpus carries thousands of
    // advisory drift notes by design, and a release must not be hostage to them.
    let (report, _) = crate::commands::check::run(ctx, false, false, false)?;
    if !report.violations.is_empty() {
        return Err(VaireError::CheckViolations(report.violations.len()));
    }

    let written = record::write(
        root,
        &ctx.config,
        version,
        bump,
        &classification,
        notes.as_deref(),
    )?;
    write_manifest_version(&ctx.repo.config_path(), version)?;

    let message = match bump {
        Some(bump) => format!("release: {package} {version} ({bump})"),
        None => format!("release: {package} {version}"),
    };
    let commit =
        crate::git::commit_paths(root, &[manifest_rel(ctx), written.path.clone()], &message)?;
    crate::git::create_tag(root, &tag, &message)?;

    Ok(ReleaseOutput::released(
        package,
        version,
        bump,
        classification,
        tag,
        written,
        commit,
        report.warnings.len(),
    )
    .with_advisories(heavily_cited))
}

/// How many inbound references make an edit worth a second look before it ships as a
/// patch. A round number rather than a computed one: the signal is "this is load-bearing
/// for a lot of readers", and the maintainer, not the threshold, makes the call.
const HEAVILY_CITED: usize = 10;

/// Changed entities that a lot of other entities point at (registry.v2.md §3.1).
///
/// The index knows its own backlinks, so a PATCH touching heavily-cited material can say
/// so. **Advisory only**: an edit to an uncited entity sails through, and the human answer
/// stands either way — this is the one place the classifier admits that structure is a
/// proxy for meaning.
fn heavily_cited(
    index: &Index,
    classification: &classify::Classification,
) -> Result<Vec<crate::output::CitedEntity>> {
    // Only a PATCH is worth questioning: a MINOR is additive, and a MAJOR is already a
    // deliberate act with notes attached.
    if classification.bump() != Some(Bump::Patch) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for id in &classification.changed {
        let inbound = index.scalar_i64(
            "SELECT count(DISTINCT from_id) FROM edges WHERE to_id = ?1 AND to_package IS NULL",
            [id.as_str()],
        )? as usize;
        if inbound >= HEAVILY_CITED {
            out.push(crate::output::CitedEntity {
                id: id.clone(),
                inbound,
            });
        }
    }
    out.sort_by(|a, b| b.inbound.cmp(&a.inbound).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

/// Ask before shipping an edit to heavily-cited material as a patch.
///
/// Skipped entirely by `--yes`, and skipped when there is no terminal to ask — a prompt
/// that blocks an automated release would be a bug, and this signal is advisory. Declining
/// is a clean refusal naming the alternative, not a failure.
fn confirm_patch(cited: &[crate::output::CitedEntity], options: &Options<'_>) -> Result<()> {
    if cited.is_empty() || options.yes {
        return Ok(());
    }
    let worst = &cited[0];
    let prompt = format!(
        "{} has {} inbound references and this would ship as a patch — continue?",
        worst.id, worst.inbound
    );
    match inquire::Confirm::new(&prompt).with_default(true).prompt() {
        Ok(true) => Ok(()),
        Ok(false) => Err(VaireError::Release(
            "stopped at your request — pass `--major --notes <file>` if this changes what \
             those references were relying on"
                .into(),
        )),
        // No terminal (CI), or the prompt was interrupted: proceed. The advisory rides
        // along in the output either way.
        Err(_) => Ok(()),
    }
}

/// The state a release may be cut from: the package's own repository, with commits, a
/// clean tree, and HEAD on the mainline.
fn preflight_git(ctx: &Ctx, options: &Options<'_>) -> Result<()> {
    let root = ctx.repo.root();
    if !ctx.repo.is_git_root() {
        return Err(VaireError::Release(
            "this package is not its own Git repository, so a release could not name a \
             commit that is only this package's — release from the package root"
                .into(),
        ));
    }
    if crate::git::head(root)?.is_none() {
        return Err(VaireError::Release(
            "this repository has no commits yet — knowledge is committed before it is \
             released"
                .into(),
        ));
    }
    // A release commit must contain the manifest and the record it just wrote, and
    // nothing else. Committing somebody's unrelated work-in-progress under a release
    // message would misattribute it forever.
    // The notes file is an input to this command, not stray work — see
    // `working_tree_dirty_except`.
    let ignore: Vec<String> = options
        .notes
        .and_then(|notes| notes.canonicalize().ok())
        .and_then(|notes| {
            let root = root.canonicalize().ok()?;
            let rel = notes.strip_prefix(&root).ok()?;
            Some(vec![rel.to_string_lossy().replace('\\', "/")])
        })
        .unwrap_or_default();
    if crate::git::working_tree_dirty_except(root, &ignore)? {
        return Err(VaireError::Release(
            "the working tree has uncommitted changes — commit or stash them first, so \
             the release commit contains only the release"
                .into(),
        ));
    }
    if options.allow_branch {
        return Ok(());
    }
    // A tag cut on a topic branch names a commit the mainline may never contain, and the
    // next release would then diff against content that never shipped.
    if let Some(default) = crate::git::default_branch(root)? {
        let current = crate::git::current_branch(root)?;
        if current.as_deref() != Some(default.as_str()) {
            let where_ = current.unwrap_or_else(|| "a detached HEAD".to_string());
            return Err(VaireError::Release(format!(
                "releases are cut from {default}, and this is {where_} — merge first, or \
                 pass `--allow-branch` if you mean it"
            )));
        }
    }
    Ok(())
}

/// Refuse before writing anything if the release could not complete.
fn preflight_writes(ctx: &Ctx, tag: &str, record_path: &Path) -> Result<()> {
    if crate::git::resolve_rev(ctx.repo.root(), tag)?.is_some() {
        return Err(VaireError::Release(format!(
            "tag {tag} already exists — a published version is immutable, so a re-release \
             is a new version rather than a replacement"
        )));
    }
    // A record the package's own include globs would never index is worse than no record:
    // it would sit in the tree describing a release nothing can query.
    let scanner = Scanner::from_config(&ctx.config)?;
    if !scanner.is_match(record_path) {
        let dir = ctx.config.release_dir.trim_end_matches('/');
        return Err(VaireError::Release(format!(
            "release records go in {dir}/, which this package's `include` globs do not \
             select — add \"{dir}/**/*.md\" to `include` in knowledge.toml, or point \
             `release_dir` somewhere already included"
        )));
    }
    Ok(())
}

/// The manifest as it stood at the baseline tag — the globs and vocabulary that decided
/// what was a node *then*. A manifest that cannot be read or parsed at that commit falls
/// back to today's, which is imperfect but strictly better than refusing to classify.
fn baseline_config(ctx: &Ctx, tag: &str) -> crate::config::Config {
    crate::git::show_many_at(ctx.repo.root(), tag, &["knowledge.toml".to_string()])
        .ok()
        .and_then(|mut texts| texts.pop().flatten())
        .and_then(|text| {
            crate::config::Config::parse(&text, "knowledge.toml at the last release").ok()
        })
        .unwrap_or_else(|| ctx.config.clone())
}

fn read_notes(path: Option<&Path>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(path)
        .map_err(|e| VaireError::Release(format!("{}: {e}", path.display())))?;
    if text.trim().is_empty() {
        return Err(VaireError::Release(format!(
            "{} is empty — the invalidated-assumptions notes are what dependents read to \
             decide whether their references still hold",
            path.display()
        )));
    }
    Ok(Some(text))
}

/// Rewrite `version` in the manifest, preserving comments and formatting — the manifest
/// is a hand-authored file that happens to have one tool-managed field.
fn write_manifest_version(manifest: &Path, version: Version) -> Result<()> {
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| VaireError::Config(format!("{}: {e}", manifest.display())))?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| VaireError::Config(format!("{}: {e}", manifest.display())))?;
    doc["version"] = value(version.to_string());
    std::fs::write(manifest, doc.to_string())?;
    Ok(())
}

fn manifest_rel(ctx: &Ctx) -> String {
    ctx.repo
        .config_path()
        .strip_prefix(ctx.repo.root())
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| "knowledge.toml".to_string())
}

/// A scratch database beside the index, removed however the run ends.
struct Scratch {
    db: PathBuf,
}

impl Scratch {
    fn new(root: &Path, name: &str) -> Result<Scratch> {
        let dir = Repo::prepare_derived_dir(root)?;
        Ok(Scratch {
            db: dir.join(format!(".{name}-{}.db", std::process::id())),
        })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = build::remove_db_files(&self.db);
    }
}
