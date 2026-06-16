//! `vaire init [path]` — scaffold a corpus.
//!
//! Discovery keys off a `.vaire/` directory (cli.md §2.1), so a brand-new corpus needs
//! one before any other command can find it. `init` writes the committed
//! `.vaire/config.toml` (the corpus marker) plus a self-contained `.vaire/.gitignore`
//! that keeps the derived index out of version control. It operates on an explicit path
//! (or the current directory) — it cannot use repo discovery, since it is what makes the
//! repo discoverable.

use std::path::Path;

use crate::error::{Result, VaireError};
use crate::output::InitOutput;

/// Default committed config (cli.md §6 defaults), annotated.
const DEFAULT_CONFIG: &str = r#"# .vaire/config.toml — committed, version-controlled (the one authored file under .vaire/).
# Everything else under .vaire/ is derived and gitignored.

# Where to look. The `id:`+`type:` pair is still what makes a file a node; these globs
# only bound the search space.
include = ["knowledge/**/*.md", "projects/**/*.md"]
exclude = ["**/node_modules/**", "**/drafts/**", "**/archive/**"]

# Type vocabulary — the `type:` field, which is also the ID prefix in `type:id`. Growable.
id_prefixes = ["person", "department", "method", "system", "event", "record", "project"]
vocabulary_strict = false

[embeddings]
# Local by default (offline, no model file). Set provider = "command" to shell out to a
# real local embedder (texts as a JSON array on stdin → vectors as JSON on stdout).
provider = "local"
dimensions = 384
"#;

/// `.vaire/.gitignore`: ignore everything derived, keep only the authored config (and
/// this file). Self-contained, so `init` need not touch the repo's root `.gitignore`.
const GITIGNORE: &str = "# Vairë — derived index, rebuildable from the corpus files.\n# Only config.toml is committed.\n*\n!.gitignore\n!config.toml\n";

pub fn run(path: Option<&Path>) -> Result<InitOutput> {
    let root = path.unwrap_or_else(|| Path::new("."));
    let vaire_dir = root.join(".vaire");
    let config_path = vaire_dir.join("config.toml");

    if config_path.exists() {
        return Err(VaireError::Usage(format!(
            "already a Vairë corpus: {} exists",
            config_path.display()
        )));
    }

    std::fs::create_dir_all(&vaire_dir)?;
    std::fs::write(&config_path, DEFAULT_CONFIG)?;
    std::fs::write(vaire_dir.join(".gitignore"), GITIGNORE)?;

    Ok(InitOutput {
        root: root.display().to_string(),
        config_path: config_path.display().to_string(),
    })
}
