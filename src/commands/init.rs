//! `vaire init [path]` — scaffold or migrate a package.
//!
//! Discovery keys off a committed `knowledge.toml` at the package root (v0.2), so a new
//! package needs one before any other command can find it. `init` writes that manifest plus
//! a self-contained `.vaire/.gitignore` that keeps the derived index out of version control.
//! If a legacy `.vaire/config.toml` is present it is **migrated** into `knowledge.toml`
//! instead. It operates on an explicit path (or the current directory) — it cannot use repo
//! discovery, since it is what makes the package discoverable.

use std::path::Path;

use crate::error::{Result, VaireError};
use crate::output::InitOutput;

/// The entity types a fresh package starts with (the old default vocabulary). Narrow this to
/// the types the package actually defines as it grows.
const DEFAULT_TYPES: &str =
    r#"["person", "department", "method", "system", "event", "record", "project"]"#;

/// `.vaire/.gitignore`: ignore everything derived (the index), keep only this file. The
/// committed manifest now lives at the root (`knowledge.toml`), not under `.vaire/`.
const GITIGNORE: &str =
    "# Vairë — derived index, rebuildable from the corpus files.\n*\n!.gitignore\n";

pub fn run(path: Option<&Path>) -> Result<InitOutput> {
    let root = path.unwrap_or_else(|| Path::new("."));
    let manifest = root.join("knowledge.toml");
    let legacy = root.join(".vaire").join("config.toml");

    if manifest.exists() {
        return Err(VaireError::Usage(format!(
            "already a Vairë package: {} exists",
            manifest.display()
        )));
    }

    std::fs::create_dir_all(root)?;
    let name = derive_name(root);

    let migrated = legacy.exists();
    let body = if migrated {
        migrate_legacy(&std::fs::read_to_string(&legacy)?, &name)?
    } else {
        default_manifest(&name)
    };
    std::fs::write(&manifest, body)?;

    // `.vaire/.gitignore` keeps the derived index untracked.
    let vaire_dir = root.join(".vaire");
    std::fs::create_dir_all(&vaire_dir)?;
    std::fs::write(vaire_dir.join(".gitignore"), GITIGNORE)?;

    // Set the migrated legacy config aside so it is not re-migrated or confused for live.
    if migrated {
        std::fs::rename(&legacy, vaire_dir.join("config.toml.migrated"))?;
    }

    Ok(InitOutput {
        root: root.display().to_string(),
        config_path: manifest.display().to_string(),
        migrated,
    })
}

/// A fresh manifest with declared identity and the starter type vocabulary.
fn default_manifest(name: &str) -> String {
    format!(
        "name = \"{name}\"\nversion = \"0.1.0\"\n\n\
         # Entity types this package defines (the `type:` field, also the id prefix in `type:id`).\n\
         types = {DEFAULT_TYPES}\n",
    )
}

/// Transform a legacy `.vaire/config.toml` into a `knowledge.toml`: rename `id_prefixes` →
/// `types`, drop `[embeddings]` (relocated to global config in a later step), and inject the
/// required `name`/`version`. Comments are not preserved (the file is regenerated).
fn migrate_legacy(legacy_text: &str, name: &str) -> Result<String> {
    let mut table: toml::Table = toml::from_str(legacy_text)
        .map_err(|e| VaireError::Config(format!("legacy .vaire/config.toml: {e}")))?;

    table.remove("embeddings");
    if let Some(prefixes) = table.remove("id_prefixes") {
        table.insert("types".to_string(), prefixes);
    }
    // Legacy `scoped_types` was a behaviour gate; scoping is now data-driven and the list is a
    // lint policy. Preserve a non-empty list as the whitelist (its "only these types" intent);
    // an empty/absent list becomes the default (permit all), so drop it.
    if let Some(scoped) = table.remove("scoped_types") {
        if scoped.as_array().is_some_and(|a| !a.is_empty()) {
            table.insert("scoped_types_whitelist".to_string(), scoped);
        }
    }
    // name/version are emitted explicitly at the top; drop any stray copies.
    table.remove("name");
    table.remove("version");

    let rest = toml::to_string_pretty(&table)
        .map_err(|e| VaireError::Config(format!("re-serialize migrated manifest: {e}")))?;
    let mut out = format!("name = \"{name}\"\nversion = \"0.1.0\"\n");
    if !rest.trim().is_empty() {
        out.push('\n');
        out.push_str(&rest);
    }
    Ok(out)
}

/// Derive a package name from the directory, sanitised to a valid slug (`[a-z][a-z0-9-]*`);
/// falls back to `corpus` when the directory name yields nothing usable.
fn derive_name(root: &Path) -> String {
    let raw = std::fs::canonicalize(root)
        .ok()
        .as_deref()
        .and_then(|p| p.file_name().map(|s| s.to_owned()))
        .or_else(|| root.file_name().map(|s| s.to_owned()))
        .and_then(|s| s.to_str().map(str::to_owned))
        .unwrap_or_default();

    let mapped: String = raw
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    // A slug must start with a letter; drop any leading non-letters, trim edges.
    let slug: String = mapped
        .trim_matches('-')
        .trim_start_matches(|c: char| !c.is_ascii_lowercase())
        .to_string();

    if slug.is_empty() {
        "corpus".to_string()
    } else {
        slug
    }
}
