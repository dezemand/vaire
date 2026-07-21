//! Graph queries backing the read commands (cli.md §3).
//!
//! Pure reads against the built index. These return the in-crate shapes the
//! `commands` modules turn into [`crate::output::Output`]s. Backlinks/traversal are
//! pure graph (no vectors); `resolve` follows `superseded_by` redirects.

use std::collections::HashSet;

use turso::Value;

use crate::error::{Result, VaireError};
use crate::index::db::{Index, col_opt_text, col_text, col_u32};
use crate::model::id::{NodeId, NodeType};

/// A resolved node location + frontmatter (cli.md §3.1).
#[derive(Debug, Clone)]
pub struct ResolvedNode {
    pub id: NodeId,
    pub node_type: NodeType,
    pub path: String,
    pub frontmatter: serde_json::Value,
    /// The requested ID, when it differed from `id` because a redirect was followed.
    pub requested_id: Option<NodeId>,
    /// The redirect target chain that was followed, if any.
    pub superseded_by: Option<NodeId>,
}

/// One inbound/outbound edge row as returned by `backlinks`/`refs` (cli.md §3.2/§3.3).
#[derive(Debug, Clone)]
pub struct EdgeRow {
    pub id: NodeId,
    pub node_type: NodeType,
    pub path: String,
    pub ref_type: String,
    pub line: u32,
    /// Shortest hop distance from the query node (always 1 for backlinks / depth-1 refs).
    pub distance: u32,
}

/// One loose end as returned by `unresolved` (cli.md §3.5).
#[derive(Debug, Clone)]
pub struct UnresolvedRow {
    pub record: NodeId,
    pub path: String,
    pub type_guess: Option<NodeType>,
    pub descriptor: String,
    pub line: u32,
}

/// A node's stored core fields.
struct Stored {
    node_type: String,
    path: String,
    frontmatter: String,
    superseded_by: Option<String>,
}

impl Index {
    fn stored(&self, id: &NodeId) -> Result<Option<Stored>> {
        self.query_opt(
            "SELECT type, path, frontmatter, superseded_by FROM nodes WHERE id = ?1",
            [id.to_string()],
            |r| {
                Ok(Stored {
                    node_type: col_text(r, 0)?,
                    path: col_text(r, 1)?,
                    frontmatter: col_text(r, 2)?,
                    superseded_by: col_opt_text(r, 3)?,
                })
            },
        )
    }

    /// `resolve <id>`: locate a node, following `superseded_by` redirects. Errors with
    /// [`VaireError::IdNotFound`] (exit `5`) if the ID is not a node.
    pub fn resolve(&self, id: &NodeId) -> Result<ResolvedNode> {
        let requested = id.clone();
        let mut current = id.clone();
        let mut seen = HashSet::new();

        loop {
            let Some(stored) = self.stored(&current)? else {
                return Err(VaireError::IdNotFound(current.to_string()));
            };
            match stored.superseded_by.as_deref().filter(|s| !s.is_empty()) {
                Some(next) if seen.insert(current.to_string()) => {
                    current = next
                        .parse()
                        .map_err(|_| VaireError::IdNotFound(next.to_string()))?;
                }
                // Terminal node (no redirect, or a redirect cycle we refuse to follow).
                _ => {
                    let followed = current != requested;
                    return Ok(ResolvedNode {
                        node_type: NodeType::new(stored.node_type),
                        path: stored.path,
                        frontmatter: frontmatter_view(&stored.frontmatter),
                        requested_id: followed.then(|| requested.clone()),
                        superseded_by: followed.then(|| current.clone()),
                        id: current,
                    });
                }
            }
        }
    }

    /// `backlinks <id>`: inbound edges (one row per edge), optionally type-filtered,
    /// sorted by referencing node id ascending.
    pub fn backlinks(
        &self,
        id: &NodeId,
        type_filter: Option<&NodeType>,
        limit: Option<usize>,
    ) -> Result<Vec<EdgeRow>> {
        let mut sql = String::from(
            "SELECT e.from_id, n.type, n.path, e.ref_type, e.line
             FROM edges e JOIN nodes n ON n.id = e.from_id
             WHERE e.to_id = ?1",
        );
        let mut params: Vec<Value> = vec![Value::from(id.to_string())];
        if let Some(t) = type_filter {
            params.push(Value::from(t.as_str().to_string()));
            sql.push_str(" AND n.type = ?2");
        }
        sql.push_str(" ORDER BY e.from_id ASC, e.line ASC");
        if let Some(n) = limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }

        self.query_rows(&sql, params, |r| {
            Ok(EdgeRow {
                id: parse_id(col_text(r, 0)?),
                node_type: NodeType::new(col_text(r, 1)?),
                path: col_text(r, 2)?,
                ref_type: col_text(r, 3)?,
                line: col_u32(r, 4)?,
                distance: 1,
            })
        })
    }

    /// `refs <id> --depth N`: outbound edges as a BFS. The result is a de-duplicated node
    /// set, each at its shortest distance, sorted by `(distance, id)`. Targets that are
    /// not nodes (dangling refs) are not traversable and are omitted (cli.md §3.3).
    pub fn refs(
        &self,
        id: &NodeId,
        depth: u32,
        type_filter: Option<&NodeType>,
    ) -> Result<Vec<EdgeRow>> {
        let mut seen: HashSet<String> = HashSet::from([id.to_string()]);
        let mut found: Vec<EdgeRow> = Vec::new();
        let mut frontier = vec![id.clone()];

        for dist in 1..=depth {
            let mut next = Vec::new();
            for node in &frontier {
                for (to_id, ref_type, line) in self.outbound(node)? {
                    if !seen.insert(to_id.to_string()) {
                        continue;
                    }
                    // Only real nodes are traversable / returned.
                    if let Some(stored) = self.stored(&to_id)? {
                        found.push(EdgeRow {
                            node_type: NodeType::new(stored.node_type),
                            path: stored.path,
                            ref_type,
                            line,
                            distance: dist,
                            id: to_id.clone(),
                        });
                        next.push(to_id);
                    }
                }
            }
            frontier = next;
        }

        if let Some(t) = type_filter {
            found.retain(|r| &r.node_type == t);
        }
        found.sort_by(|a, b| a.distance.cmp(&b.distance).then(a.id.cmp(&b.id)));
        Ok(found)
    }

    /// Outbound edges of one node, in stable order, as `(to_id, ref_type, line)`.
    fn outbound(&self, from: &NodeId) -> Result<Vec<(NodeId, String, u32)>> {
        self.query_rows(
            "SELECT to_id, ref_type, line FROM edges WHERE from_id = ?1 ORDER BY line, to_id",
            [from.to_string()],
            |r| Ok((parse_id(col_text(r, 0)?), col_text(r, 1)?, col_u32(r, 2)?)),
        )
    }

    /// `unresolved`: every `[[?...]]` currently in the corpus, derived fresh from the
    /// indexed rows (no stored queue — design.md §8). Sorted by `(source path, line)`.
    /// `--type T` matches the `?type` hint exactly, so `[[?: …]]` (null hint) appears
    /// only when no type filter is given (cli.md §3.5).
    pub fn unresolved(
        &self,
        type_filter: Option<&NodeType>,
        scope: Option<&NodeId>,
        scope_field: &str,
    ) -> Result<Vec<UnresolvedRow>> {
        let mut sql = String::from(
            "SELECT record_id, type_guess, descriptor, source_file, line FROM unresolved WHERE 1=1",
        );
        let mut params: Vec<Value> = Vec::new();
        if let Some(t) = type_filter {
            params.push(Value::from(t.as_str().to_string()));
            sql.push_str(&format!(" AND type_guess = ?{}", params.len()));
        }
        if let Some(s) = scope {
            params.push(Value::from(scope_field.to_string()));
            let field_idx = params.len();
            params.push(Value::from(s.to_string()));
            let scope_idx = params.len();
            sql.push_str(&format!(
                " AND record_id IN (SELECT from_id FROM edges WHERE ref_type = ?{field_idx} AND to_id = ?{scope_idx})"
            ));
        }
        sql.push_str(" ORDER BY source_file ASC, line ASC");

        self.query_rows(&sql, params, |r| {
            Ok(UnresolvedRow {
                record: parse_id(col_text(r, 0)?),
                type_guess: col_opt_text(r, 1)?.map(NodeType::new),
                descriptor: col_text(r, 2)?,
                path: col_text(r, 3)?,
                line: col_u32(r, 4)?,
            })
        })
    }
}

/// Parse a stored ID string back into a [`NodeId`]. Lenient by design: a declared id
/// that falls outside the strict reference grammar still indexes (files are truth) and
/// must round-trip unchanged — see [`NodeId::parse_stored`].
fn parse_id(s: String) -> NodeId {
    NodeId::parse_stored(&s)
}

/// The frontmatter view returned by `resolve`: the stored JSON minus `id`/`type`, which
/// are surfaced as top-level fields (cli.md §3.1).
fn frontmatter_view(json: &str) -> serde_json::Value {
    let mut value: serde_json::Value =
        serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.remove("id");
        obj.remove("type");
    }
    value
}
