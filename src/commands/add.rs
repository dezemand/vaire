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

    // Validate the link target *before* touching the manifest, so a bad --link leaves
    // everything unchanged.
    let link_target = match link {
        Some(path) => Some(validate_link_target(&name, path)?),
        None => None,
    };

    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| VaireError::Config(format!("{}: {e}", manifest.display())))?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| VaireError::Config(format!("{}: {e}", manifest.display())))?;

    // Ensure a `[dependencies]` table exists, then set the constraint (creating or updating).
    let deps = doc["dependencies"].or_insert(toml_edit::table());
    let table = deps
        .as_table_mut()
        .ok_or_else(|| VaireError::Config("`dependencies` is not a table".to_string()))?;
    let updated = table.contains_key(&name);
    table[&name] = value(&constraint);

    std::fs::write(&manifest, doc.to_string())?;

    let root = manifest
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let linked = match link_target {
        Some(target) => Some(create_link(&root, &name, &target)?),
        None => None,
    };

    Ok(AddOutput {
        name,
        constraint,
        config_path: manifest.display().to_string(),
        updated,
        linked,
    })
}

/// Canonicalize and verify a `--link` target: it must exist and be a package whose
/// manifest declares `name`. All failures are usage errors (exit 2) — bad argument, and
/// nothing has been written yet.
fn validate_link_target(name: &str, path: &Path) -> Result<PathBuf> {
    let target = std::fs::canonicalize(path)
        .map_err(|e| VaireError::Usage(format!("--link {}: {e}", path.display())))?;
    let manifest = target.join("knowledge.toml");
    if !manifest.is_file() {
        return Err(VaireError::Usage(format!(
            "--link {}: not a package (no knowledge.toml)",
            path.display()
        )));
    }
    let config = Config::load(&manifest)
        .map_err(|e| VaireError::Usage(format!("--link {}: {e}", path.display())))?;
    if config.name != name {
        return Err(VaireError::Usage(format!(
            "--link {}: that package declares name '{}', not '{name}' — identity is declared, never path-derived",
            path.display(),
            config.name
        )));
    }
    Ok(target)
}

/// Create (or replace) the `.vaire/packages/<name>` symlink. The stored link target is
/// relative to the packages dir when possible (portable if the checkout and the target
/// move together), else absolute. An existing symlink is replaced; a **real** directory
/// at that entry is never touched (it may be v0.3-installed content — refuse).
fn create_link(root: &Path, name: &str, target: &Path) -> Result<String> {
    let packages = Repo::packages_dir_at(root);
    std::fs::create_dir_all(&packages)?;
    // `.vaire/` may predate this command or be freshly created here; either way the
    // derived dir must carry its self-contained gitignore (design.md §9).
    let gitignore = root.join(".vaire").join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(
            &gitignore,
            "# Vairë — derived index, rebuildable from the corpus files.\n*\n!.gitignore\n",
        )?;
    }

    let entry = packages.join(name);
    match std::fs::symlink_metadata(&entry) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(&entry)?,
        Ok(_) => {
            return Err(VaireError::Usage(format!(
                "{} exists and is a real directory (not a link) — refusing to replace it",
                entry.display()
            )));
        }
        Err(_) => {}
    }

    let stored =
        crate::workspace::relative_to(target, &packages).unwrap_or_else(|| target.to_path_buf());
    symlink_dir(&stored, &entry)?;
    Ok(stored.display().to_string())
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
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
