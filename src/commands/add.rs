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

use crate::config::{is_caret_major, is_slug};
use crate::corpus::repo::Repo;
use crate::error::{Result, VaireError};
use crate::output::AddOutput;
use crate::workspace::discover;
use crate::workspace::link::{self, LinkError, LinkPlan};

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

    let mut linked = link_plan.map(link::commit).transpose()?;

    // No explicit `--link`: satisfy the dependency from the local-packages root, exactly
    // as the ensure pass would. Declaring is what this command does; wiring is a
    // convenience on top, so nothing here can fail the run — a name that cannot be found
    // (or is ambiguous) comes back as a note.
    let mut discovered = false;
    let mut note = None;
    if linked.is_none() {
        // The manifest is already written, so nothing past this point may fail the run —
        // including an unreadable user config, which becomes a note like any other reason
        // the dependency could not be wired.
        match crate::userconfig::UserConfig::load() {
            Ok(user) => {
                let satisfied =
                    discover::satisfy_name(&root, &name, user.packages.local.as_deref());
                if let Some(dep) = satisfied.linked.first() {
                    linked = Some(dep.target.clone());
                    discovered = true;
                } else {
                    note = satisfied
                        .notes
                        .get(&name)
                        .cloned()
                        .or_else(|| satisfied.warnings.first().cloned());
                }
            }
            Err(e) => note = Some(format!("could not read the user config: {e}")),
        }
    }

    Ok(AddOutput {
        name,
        constraint,
        config_path: manifest.display().to_string(),
        updated,
        linked,
        discovered,
        note,
    })
}

/// Stage an explicit `--link`: shared validation ([`crate::workspace::link::plan`]) with
/// this command's phrasing — every failure is a usage error (exit 2) naming the flag.
fn plan_link(root: &Path, name: &str, path: &Path) -> Result<LinkPlan> {
    link::plan(root, name, path).map_err(|e| match e {
        LinkError::Target(m) => VaireError::Usage(format!("--link {}: {m}", path.display())),
        LinkError::Entry(m) => VaireError::Usage(m),
    })
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
