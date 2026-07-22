//! `vaire add <pkg>[@^N] [--link <path>]` — declare a dependency in `knowledge.toml`
//! and, with `--link`, wire up where it lives (cli.md §4.2a, §6.5).
//!
//! Edits the `[dependencies]` table of the committed manifest, **preserving formatting and
//! comments** (via `toml_edit`) — the manifest is user-authored, so a full re-serialize
//! (as `init`'s legacy migration does) would be wrong here. Like `init`/`configure`, this
//! mutates a file and needs no index, so it runs before `Ctx` is built; it discovers the
//! package root by walking up to the nearest `knowledge.toml` (or honours `--config`).
//!
//! `^MAJOR` is the only legal constraint (packages.md §6); the default is `^1`. Adding a
//! package already present **updates** its constraint in place (idempotent).
//!
//! `--link` creates (or replaces) the `.vaire/packages/<name>` symlink. The manifest never
//! carries the path — the committed contract stays machine-independent; the link is
//! per-checkout state under gitignored `.vaire/`. The target must be a package whose
//! `knowledge.toml` declares this same `name` (identity is declared, never path-derived).

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, value};

use crate::config::{Config, is_caret_major, is_slug};
use crate::corpus::repo::Repo;
use crate::error::{Result, VaireError};
use crate::output::AddOutput;

/// Add (or update) a dependency. `spec` is `<name>` or `<name>@<constraint>` (e.g.
/// `acme-core`, `acme-core@^2`); `link` is the `--link <path>` target, resolved against
/// the current working directory. `repo_override`/`config_override` come from the global
/// `--repo`/`--config` flags.
pub fn run(
    repo_override: Option<&Path>,
    config_override: Option<&Path>,
    spec: &str,
    link: Option<&Path>,
) -> Result<AddOutput> {
    let (name, constraint) = parse_spec(spec)?;

    let manifest = resolve_manifest(repo_override, config_override)?;
    let root = manifest
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| VaireError::Config(format!("{}: {e}", manifest.display())))?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| VaireError::Config(format!("{}: {e}", manifest.display())))?;

    // A package cannot depend on itself — bare references are already local. Caught
    // here so the manifest is never touched (resolution would reject it anyway).
    if doc.get("name").and_then(|i| i.as_str()) == Some(name.as_str()) {
        return Err(VaireError::Usage(format!(
            "package '{name}' cannot depend on itself; bare references are already local"
        )));
    }

    // Plan the link fully *before* touching the manifest — target validation AND the
    // replaceability check — so any --link failure leaves everything unchanged.
    let link_plan = match link {
        Some(path) => Some(plan_link(&root, &name, path)?),
        None => None,
    };

    // Ensure a `[dependencies]` table exists, then set the constraint (creating or updating).
    let deps = doc["dependencies"].or_insert(toml_edit::table());
    let table = deps
        .as_table_mut()
        .ok_or_else(|| VaireError::Config("`dependencies` is not a table".to_string()))?;
    let updated = table.contains_key(&name);
    table[&name] = value(&constraint);

    std::fs::write(&manifest, doc.to_string())?;

    let linked = link_plan.map(commit_link).transpose()?;

    Ok(AddOutput {
        name,
        constraint,
        config_path: manifest.display().to_string(),
        updated,
        linked,
    })
}

/// A fully-validated link, ready to commit: everything that can be *rejected* has been.
struct LinkPlan {
    entry: PathBuf,
    /// The symlink target as it will be stored (relative to the packages dir when the
    /// paths share a prefix — portable if the checkout and target move together).
    stored: PathBuf,
}

/// Validate and stage a `--link`: canonicalize the target, require a package declaring
/// `name` (identity is declared, never path-derived), prepare `.vaire/packages/` (with
/// the derived-dir gitignore), and refuse a **real directory** at the entry (it may be
/// installed content). All failures are usage errors (exit 2) — nothing written yet
/// except derived dirs.
fn plan_link(root: &Path, name: &str, path: &Path) -> Result<LinkPlan> {
    let target = std::fs::canonicalize(path)
        .map_err(|e| VaireError::Usage(format!("--link {}: {e}", path.display())))?;
    let target_manifest = target.join("knowledge.toml");
    if !target_manifest.is_file() {
        return Err(VaireError::Usage(format!(
            "--link {}: not a package (no knowledge.toml)",
            path.display()
        )));
    }
    let config = Config::load(&target_manifest)
        .map_err(|e| VaireError::Usage(format!("--link {}: {e}", path.display())))?;
    if config.name != name {
        return Err(VaireError::Usage(format!(
            "--link {}: that package declares name '{}', not '{name}' — identity is declared, never path-derived",
            path.display(),
            config.name
        )));
    }

    let packages = Repo::prepare_packages_dir(root)?;
    Repo::ensure_derived_gitignore(&Repo::prepare_derived_dir(root)?)?;
    // Canonicalize so the relative computation sees the same prefix shape as the
    // (already canonical) target — e.g. macOS's /var → /private/var.
    let entry = packages.join(name);
    if let Ok(meta) = std::fs::symlink_metadata(&entry)
        && !meta.file_type().is_symlink()
    {
        return Err(VaireError::Usage(format!(
            "{} exists and is a real directory (not a link) — refusing to replace it",
            entry.display()
        )));
    }

    let stored =
        crate::workspace::relative_to(&target, &packages).unwrap_or_else(|| target.clone());
    Ok(LinkPlan { entry, stored })
}

/// Commit a staged link: create the symlink at a temporary name, then rename over the
/// entry — atomically replacing an existing symlink, so a reader never observes the
/// entry missing.
fn commit_link(plan: LinkPlan) -> Result<String> {
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

/// Parse `<name>[@<constraint>]`; default constraint `^1`. Validates the name slug and the
/// `^MAJOR` constraint form up front, so a bad argument is a usage error (exit 2), not a
/// malformed manifest.
fn parse_spec(spec: &str) -> Result<(String, String)> {
    let (name, constraint) = match spec.split_once('@') {
        Some((n, c)) => (n, c),
        None => (spec, "^1"),
    };
    if !is_slug(name) {
        return Err(VaireError::Usage(format!(
            "invalid package name '{name}' (must match [a-z][a-z0-9-]*)"
        )));
    }
    if !is_caret_major(constraint) {
        return Err(VaireError::Usage(format!(
            "invalid constraint '{constraint}' (only ^MAJOR is allowed, e.g. ^1)"
        )));
    }
    Ok((name.to_string(), constraint.to_string()))
}

/// The manifest to edit: an explicit `--config` path, else the discovered package's
/// `knowledge.toml`.
fn resolve_manifest(
    repo_override: Option<&Path>,
    config_override: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(p) = config_override {
        return Ok(p.to_path_buf());
    }
    let cwd = std::env::current_dir()?;
    Ok(Repo::discover(repo_override, &cwd)?.config_path())
}
