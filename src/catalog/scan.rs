//! Bulk import — the discovery walk, demoted from resolution machinery to an import tool.
//!
//! v0.2.0 walked a configured root on every maintain command to answer "is there something
//! called this?". The catalog answers that from a lookup instead, so the walk survives here
//! and only here: `vaire catalog scan <dir>` records everything under a directory once, and
//! nothing walks anything again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config::Config;

/// How deep under the root a package may sit. Knowledge bases are often one component of a
/// larger repository (`<root>/<repo>/docs/kb`), so one level is not enough — but this must
/// not turn into a walk of an entire home directory.
const MAX_DEPTH: usize = 4;

/// Directories never worth descending — heavy build/vendor trees that never hold a package.
/// Dotted names are skipped separately.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "dist", "build", "venv"];

/// How many directories one scan may visit. `MAX_DEPTH` bounds how *deep* the walk goes but
/// not how *wide*; a root pointed at something enormous would otherwise walk all of it.
const MAX_VISITED: usize = 10_000;

/// One package the walk found.
pub struct Found {
    pub path: PathBuf,
    pub config: Config,
}

/// What a walk turned up.
#[derive(Default)]
pub struct Walk {
    pub found: Vec<Found>,
    /// Directories holding an unreadable `knowledge.toml` — surfaced so a malformed
    /// manifest does not look like an absent package.
    pub unreadable: Vec<String>,
    /// The walk hit [`MAX_VISITED`] and stopped early, so "not found" may be wrong.
    pub truncated: bool,
}

/// Walk `root` for packages. A directory holding a `knowledge.toml` is a package: it is
/// recorded and **not** descended into (packages do not nest). Canonical paths are visited
/// at most once, so a symlink pointing back inside terminates and never doubles a package
/// into a false ambiguity.
pub fn walk(root: &Path) -> Walk {
    let mut out = Walk::default();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];

    while let Some((dir, depth)) = stack.pop() {
        if seen.len() >= MAX_VISITED {
            out.truncated = true;
            break;
        }
        let Ok(dir) = std::fs::canonicalize(&dir) else {
            continue;
        };
        if !seen.insert(dir.clone()) {
            continue;
        }
        let manifest = dir.join("knowledge.toml");
        if manifest.is_file() {
            match Config::load(&manifest) {
                Ok(config) => out.found.push(Found { path: dir, config }),
                Err(e) => out.unreadable.push(format!("{}: {e}", dir.display())),
            }
            continue;
        }
        if depth == MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                continue;
            }
            // `is_dir` follows symlinks — a root of symlinks into checkouts elsewhere is a
            // normal way to keep this directory.
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            }
        }
    }
    out.found.sort_by(|a, b| a.path.cmp(&b.path));
    out
}
