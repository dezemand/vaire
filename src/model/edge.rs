//! Edges — the rows of the graph (design.md §9 storage, cli.md §3.2/§3.3).
//!
//! Only **resolved** references become edges. The edges table is
//! `(from_id, to_id, ref_type, source_file, line)`; [`Edge`] is its in-memory form.

use crate::model::id::NodeId;

/// Where an edge came from — its `ref_type` in the spec's JSON (cli.md §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefOrigin {
    /// A frontmatter edge-list key, e.g. `participants`, `references`, `project`.
    Frontmatter(String),
    /// An inline `[[...]]` wikilink in prose.
    Inline,
}

impl RefOrigin {
    /// The string written into the `ref_type` column / JSON field.
    pub fn as_ref_type(&self) -> &str {
        match self {
            RefOrigin::Frontmatter(key) => key,
            RefOrigin::Inline => "inline",
        }
    }
}

/// One outbound edge from a node, with its provenance (source file + 1-based line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub origin: RefOrigin,
    /// Repo-root-relative POSIX path of the file the edge was found in.
    pub source_file: String,
    /// 1-based source line.
    pub line: u32,
}
