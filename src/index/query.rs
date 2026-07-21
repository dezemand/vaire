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

/// A node's stored core fields. `pub(crate)` so the workspace resolver can compose
/// per-package lookups without re-following redirects locally.
pub(crate) struct Stored {
    pub(crate) node_type: String,
    pub(crate) path: String,
    pub(crate) frontmatter: String,
    pub(crate) superseded_by: Option<String>,
}

impl Index {
    pub(crate) fn stored(&self, id: &NodeId) -> Result<Option<Stored>> {
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
            // Local inbound edges only: a cross-package edge's bare to_id could coincide
            // with this local id but points at another package's node, not this one.
            "SELECT e.from_id, n.type, n.path, e.ref_type, e.line
             FROM edges e JOIN nodes n ON n.id = e.from_id
             WHERE e.to_package IS NULL AND e.to_id = ?1",
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

    /// Inbound edges in THIS index that point at another package's node: rows whose
    /// `to_package` is `alias` and whose bare `to_id` matches. The cross-package
    /// composition (`workspace::resolver::backlinks`) merges these per member.
    pub(crate) fn backlinks_via(
        &self,
        alias: &str,
        bare_id: &str,
        type_filter: Option<&NodeType>,
        limit: Option<usize>,
    ) -> Result<Vec<EdgeRow>> {
        let mut sql = String::from(
            "SELECT e.from_id, n.type, n.path, e.ref_type, e.line
             FROM edges e JOIN nodes n ON n.id = e.from_id
             WHERE e.to_package = ?1 AND e.to_id = ?2",
        );
        let mut params: Vec<Value> = vec![
            Value::from(alias.to_string()),
            Value::from(bare_id.to_string()),
        ];
        if let Some(t) = type_filter {
            params.push(Value::from(t.as_str().to_string()));
            sql.push_str(" AND n.type = ?3");
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

    /// Outbound edges of one node, in stable order, as `(to, ref_type, line)`. A
    /// cross-package target regains its `@pkg/` qualifier; following it across the
    /// package boundary is the workspace resolver's job (`workspace::resolver::refs`).
    pub(crate) fn outbound(&self, from: &NodeId) -> Result<Vec<(NodeId, String, u32)>> {
        self.query_rows(
            "SELECT to_id, to_package, ref_type, line FROM edges
             WHERE from_id = ?1 ORDER BY line, to_id",
            [from.to_string()],
            |r| {
                let mut to = parse_id(col_text(r, 0)?);
                if let Some(pkg) = col_opt_text(r, 1)? {
                    to = to.with_package(pkg);
                }
                Ok((to, col_text(r, 2)?, col_u32(r, 3)?))
            },
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
                " AND record_id IN (SELECT from_id FROM edges WHERE to_package IS NULL AND ref_type = ?{field_idx} AND to_id = ?{scope_idx})"
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
pub(crate) fn frontmatter_view(json: &str) -> serde_json::Value {
    let mut value: serde_json::Value =
        serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.remove("id");
        obj.remove("type");
    }
    value
}
