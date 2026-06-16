//! Frontmatter parsing and node assembly (design.md §5, §9).
//!
//! A file is a node iff its YAML frontmatter carries **both** an `id:` (a bare local
//! slug) and a `type:`. The node's address is the composition `type:id` — e.g.
//! `id: hr` + `type: department` ⇒ `department:hr` (design.md §4). The `type:` field is
//! authoritative; there is no prefix-in-`id:` to keep in sync.
//!
//! This module splits the `---`-fenced YAML block from the prose, parses it, and — when
//! both `id:` and `type:` are present — assembles a [`Node`] by combining the
//! frontmatter edge-list with inline wikilinks scanned from the prose.

use std::collections::BTreeMap;

use crate::model::edge::{Edge, RefOrigin};
use crate::model::id::{NodeId, NodeType};
use crate::model::node::Node;
use crate::model::reference::Reference;

/// Frontmatter fields that are never graph edges: `id`/`type` (they compose the node's
/// own address) and `superseded_by` (a redirect, handled separately — design.md §8).
/// Plus `name`/`aliases`, which are **display** fields — never references — so a value
/// that happens to contain a colon (a title like `Workshop D: Agents`) is not mistaken
/// for a `type:id`.
pub const NON_EDGE_KEYS: &[&str] = &["id", "type", "superseded_by", "name", "aliases"];

/// The raw split of a Markdown file into its parsed frontmatter and prose body.
pub struct Document {
    pub frontmatter: BTreeMap<String, serde_yaml::Value>,
    /// File line (1-based) of each top-level frontmatter key, so frontmatter edges can
    /// report a source line (cli.md §3.2).
    pub frontmatter_lines: BTreeMap<String, u32>,
    /// The Markdown body below the closing `---`.
    pub prose: String,
    /// File line (1-based) the prose body starts on, so inline wikilink and section
    /// line numbers come out absolute.
    pub prose_start_line: u32,
}

/// Split a file's contents into frontmatter + prose. Returns `None` when there is no
/// leading `---` fence with a matching close, or the block is not a YAML mapping — in
/// which case the file is not a node and is ignored (frontmatter-driven discovery).
pub fn split(contents: &str) -> Option<Document> {
    let mut lines = contents.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }

    // Collect frontmatter lines until the closing fence. Line 1 was the opening `---`,
    // so frontmatter content begins at file line 2.
    let mut fm_lines: Vec<&str> = Vec::new();
    let mut closing: Option<u32> = None;
    let mut line_no = 1u32;
    for line in lines {
        line_no += 1;
        if line.trim() == "---" {
            closing = Some(line_no);
            break;
        }
        fm_lines.push(line);
    }
    let closing = closing?; // no closing fence ⇒ not a node

    let fm_text = fm_lines.join("\n");
    let frontmatter: BTreeMap<String, serde_yaml::Value> = serde_yaml::from_str(&fm_text).ok()?;

    // Map each top-level key (column 0, `key:`) to its file line.
    let mut frontmatter_lines = BTreeMap::new();
    for (i, raw) in fm_lines.iter().enumerate() {
        if raw.starts_with(char::is_whitespace) {
            continue; // nested value, not a top-level key
        }
        if let Some((k, _)) = raw.split_once(':') {
            let key = k.trim();
            if !key.is_empty() {
                frontmatter_lines
                    .entry(key.to_string())
                    .or_insert(2 + i as u32);
            }
        }
    }

    let prose = contents
        .lines()
        .skip(closing as usize) // skip through the closing fence line
        .collect::<Vec<_>>()
        .join("\n");

    Some(Document {
        frontmatter,
        frontmatter_lines,
        prose,
        prose_start_line: closing + 1,
    })
}

/// Compose a node's address from its frontmatter: `type:id`. Returns `None` unless both
/// a non-empty `type:` and a non-empty `id:` slug are present (design.md §4, §9).
pub fn node_id(frontmatter: &BTreeMap<String, serde_yaml::Value>) -> Option<NodeId> {
    let ty = frontmatter.get("type")?.as_str()?.trim();
    let slug = frontmatter.get("id")?.as_str()?.trim();
    if ty.is_empty() || slug.is_empty() {
        return None;
    }
    Some(NodeId::new(NodeType::new(ty), slug))
}

/// Assemble a [`Node`] from a split document at `rel_path`. Returns `None` when the
/// frontmatter lacks the `id:`+`type:` pair (frontmatter-driven discovery, design.md §9).
pub fn to_node(rel_path: &str, doc: Document) -> Option<Node> {
    let id = node_id(&doc.frontmatter)?;

    let mut edges = Vec::new();
    let mut unresolved = Vec::new();

    // 1. Frontmatter edge-list: a field (other than NON_EDGE_KEYS) whose value parses as
    //    a composed `type:id` becomes an edge keyed by its field name (design.md §5); one
    //    that parses as `?type: descriptor` becomes a loose end (cli.md §6.3).
    collect_frontmatter_refs(
        &id,
        rel_path,
        &doc.frontmatter,
        &doc.frontmatter_lines,
        &mut edges,
        &mut unresolved,
    );

    // 2. Inline wikilinks: resolved ones become inline edges; unresolved ones land in
    //    the loose-ends list (never edges — cli.md §3.3).
    for (reference, line) in super::wikilink::scan(&doc.prose, doc.prose_start_line) {
        match reference {
            Reference::Resolved { target, .. } => edges.push(Edge {
                from: id.clone(),
                to: target,
                origin: RefOrigin::Inline,
                source_file: rel_path.to_string(),
                line,
            }),
            unresolved_ref @ Reference::Unresolved { .. } => {
                unresolved.push((unresolved_ref, line))
            }
        }
    }

    Some(Node {
        id,
        path: rel_path.to_string(),
        frontmatter: doc.frontmatter,
        prose: doc.prose,
        edges,
        unresolved,
    })
}

/// Pull references out of the structured frontmatter edge-list. A scalar or sequence-item
/// value that parses as a composed `type:id` becomes an edge keyed by its field; one that
/// parses as `?type: descriptor` (the §6 unresolved syntax, *minus* the inline `[[ ]]`
/// brackets) becomes a loose end. Stray `[[ ]]` around a value are tolerated and stripped,
/// but `vaire check` flags them (cli.md §6.3). Anything else is ignored.
fn collect_frontmatter_refs(
    from: &NodeId,
    rel_path: &str,
    frontmatter: &BTreeMap<String, serde_yaml::Value>,
    frontmatter_lines: &BTreeMap<String, u32>,
    edges: &mut Vec<Edge>,
    unresolved: &mut Vec<(Reference, u32)>,
) {
    use serde_yaml::Value;
    for (key, value) in frontmatter {
        if NON_EDGE_KEYS.contains(&key.as_str()) {
            continue;
        }
        let line = frontmatter_lines.get(key).copied().unwrap_or(0);
        let mut handle = |s: &str| {
            // Frontmatter references are bare; tolerate stray inline-style `[[ ]]`.
            let inner = s.trim();
            let inner = inner
                .strip_prefix("[[")
                .and_then(|x| x.strip_suffix("]]"))
                .map(str::trim)
                .unwrap_or(inner);
            match Reference::parse_inner(inner) {
                Some(Reference::Resolved { target, .. }) => edges.push(Edge {
                    from: from.clone(),
                    to: target,
                    origin: RefOrigin::Frontmatter(key.clone()),
                    source_file: rel_path.to_string(),
                    line,
                }),
                Some(loose @ Reference::Unresolved { .. }) => unresolved.push((loose, line)),
                None => {}
            }
        };
        match value {
            Value::String(s) => handle(s),
            Value::Sequence(seq) => {
                for item in seq {
                    if let Some(s) = item.as_str() {
                        handle(s);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: &str = "\
---
id: 2026-06-10-broker-sync
type: record
project: project:atlas-2026-q2
date: 2026-06-10
participants: [person:jane-doe, department:logistics]
references: [method:event-sourcing, system:ingest-api]
---
# Broker sync, 2026-06-10

[[person:jane-doe]] walked [[department:logistics]] through it.
[[?person: someone from logistics]] raised concerns.
";

    #[test]
    fn split_separates_frontmatter_and_prose() {
        let doc = split(RECORD).expect("has frontmatter");
        // The `id:` field is the bare local slug; the address composes with `type:`.
        assert_eq!(
            doc.frontmatter.get("id").unwrap().as_str().unwrap(),
            "2026-06-10-broker-sync"
        );
        // Opening fence is line 1; `id:` is line 2.
        assert_eq!(doc.frontmatter_lines["id"], 2);
        assert_eq!(doc.frontmatter_lines["participants"], 6);
        // Closing fence is line 8; prose starts at line 9.
        assert_eq!(doc.prose_start_line, 9);
        assert!(doc.prose.starts_with("# Broker sync"));
    }

    #[test]
    fn node_id_composes_type_and_slug() {
        let doc = split(RECORD).unwrap();
        assert_eq!(
            node_id(&doc.frontmatter).unwrap().to_string(),
            "record:2026-06-10-broker-sync"
        );
    }

    #[test]
    fn non_node_without_frontmatter() {
        assert!(split("just prose, no fence").is_none());
        assert!(split("---\nno close fence\n").is_none());
        // Has frontmatter but no `type:` ⇒ not a node.
        assert!(to_node("x.md", split("---\nid: lonely\n---\n# x\n").unwrap()).is_none());
    }

    #[test]
    fn to_node_collects_frontmatter_and_inline_edges() {
        let node = to_node("rec.md", split(RECORD).unwrap()).unwrap();
        assert_eq!(node.id.to_string(), "record:2026-06-10-broker-sync");

        // Frontmatter edges: project + 2 participants + 2 references = 5.
        // Inline edges: person:jane-doe, department:logistics = 2. Total 7.
        assert_eq!(node.edges.len(), 7);

        // `id`/`type` compose the node's own address and are never edges.
        assert!(
            node.edges
                .iter()
                .all(|e| e.to.to_string() != "record:2026-06-10-broker-sync")
        );

        // Frontmatter `project` edge carries its field name as ref_type.
        let project = node
            .edges
            .iter()
            .find(|e| e.to.to_string() == "project:atlas-2026-q2")
            .unwrap();
        assert_eq!(project.origin.as_ref_type(), "project");

        // The one unresolved reference is captured, not turned into an edge.
        assert_eq!(node.unresolved.len(), 1);
    }
}
