//! A node — any `.md` file whose frontmatter carries a typed `id:` (design.md §9).
//!
//! Discovery is **by frontmatter, not path**: a file is a node iff it has a typed
//! `id:`. Everything else is prose Vairë ignores (optionally FTS-only). The node
//! carries its parsed frontmatter, its prose body, and its outbound references — both
//! the resolved edges and the unresolved loose ends.

use std::collections::BTreeMap;

use crate::model::edge::Edge;
use crate::model::id::{NodeId, NodeType};
use crate::model::reference::Reference;

/// An in-memory node parsed from a single corpus file.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    /// Repo-root-relative POSIX path.
    pub path: String,

    /// Raw frontmatter as an ordered map of YAML scalars/sequences, kept generic so
    /// Vairë stays type-agnostic. `name`, `aliases`, `status`, `project`,
    /// `superseded_by`, etc. are read out of here by convention, not by schema.
    pub frontmatter: BTreeMap<String, serde_yaml::Value>,

    /// The Markdown body below the frontmatter (used for FTS + embeddings).
    pub prose: String,

    /// Resolved outbound references (frontmatter edge-list + inline), as edges.
    pub edges: Vec<Edge>,

    /// Unresolved references found inline — the work list for the §8 creation pass.
    /// Each is paired with the 1-based line it was found on.
    pub unresolved: Vec<(Reference, u32)>,
}

impl Node {
    pub fn node_type(&self) -> &NodeType {
        &self.id.node_type
    }

    /// The default display name for references to this node (design.md §6), resolved by
    /// fallback: the `name:` field, else the sole `# H1` title in the prose, else the
    /// filename without extension. (A missing or *ambiguous* — i.e. not exactly one — H1
    /// skips to the filename.)
    pub fn display_name(&self) -> String {
        if let Some(name) = self.frontmatter.get("name").and_then(|v| v.as_str()) {
            let name = name.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
        if let Some(title) = sole_h1(&self.prose) {
            return title;
        }
        file_stem(&self.path)
    }

    /// `superseded_by: <id>` redirect target, if present (design.md §8).
    pub fn superseded_by(&self) -> Option<NodeId> {
        self.frontmatter
            .get("superseded_by")?
            .as_str()?
            .parse()
            .ok()
    }

    /// The `aliases:` list — high-precision input to reference resolution (design.md §8).
    pub fn aliases(&self) -> Vec<String> {
        match self.frontmatter.get("aliases") {
            Some(serde_yaml::Value::Sequence(seq)) => seq
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// The text of the *sole* level-1 (`# `) heading in `prose`, or `None` if there are zero
/// or more than one (ambiguous). Fenced code blocks are ignored.
fn sole_h1(prose: &str) -> Option<String> {
    let mut in_fence = false;
    let mut found: Option<String> = None;
    for line in prose.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if title.is_empty() {
                continue;
            }
            if found.is_some() {
                return None; // more than one H1 → ambiguous
            }
            found = Some(title.to_string());
        }
    }
    found
}

/// The filename without its extension (`a/b/jane-doe.md` → `jane-doe`).
fn file_stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}
