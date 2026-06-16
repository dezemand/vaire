//! Typed node IDs (design.md §6, §10).
//!
//! An ID is `<type>:<slug>` — e.g. `person:jane-doe`, `record:2026-06-10-broker-sync`.
//! The prefix is the **authoritative type** (the frontmatter `type:` field is only a
//! readable echo Vairë validates against). The vocabulary is *growable*, so the type
//! is an open string newtype, not a closed enum — a prefix Vairë has never seen is
//! still a valid node type (design.md §10, "Vocabulary will grow").

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A node type, i.e. the prefix portion of an ID. Open by design.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeType(pub String);

impl NodeType {
    pub fn new(s: impl Into<String>) -> Self {
        NodeType(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A node identity. The node's own ID is `<type>:<slug>` (the last path segment); a
/// **scoped** ID prepends a container path — `<scope>/<type>:<slug>`, where `scope` is
/// the container's ID (one level today: a project, e.g.
/// `project:atlas-2026-q2/record:standup`). `node_type`/`slug` always describe the node
/// itself; `scope` is the prefix. See cli.md §6.1.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeId {
    pub node_type: NodeType,
    pub slug: String,
    pub scope: Option<String>,
}

impl NodeId {
    pub fn new(node_type: NodeType, slug: impl Into<String>) -> Self {
        NodeId {
            node_type,
            slug: slug.into(),
            scope: None,
        }
    }

    /// The type of this node, from its own (last-segment) prefix.
    pub fn node_type(&self) -> &NodeType {
        &self.node_type
    }

    /// The scope prefix (the container's ID), or `None` for an unscoped/global node.
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    /// The node's own local slug (never includes the scope).
    pub fn local(&self) -> &str {
        &self.slug
    }

    /// The node's own ID without any scope prefix (`project:a/record:b` → `record:b`).
    pub fn local_id(&self) -> String {
        format!("{}:{}", self.node_type, self.slug)
    }

    /// Set (or replace) the scope prefix.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope {
            Some(scope) => write!(f, "{}/{}:{}", scope, self.node_type, self.slug),
            None => write!(f, "{}:{}", self.node_type, self.slug),
        }
    }
}

/// Parse a node ID. A trailing `/` segment is the node's own `type:slug`; anything
/// before the last `/` is the scope prefix. With no `/`, the whole string is `type:slug`.
impl FromStr for NodeId {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (scope, node_part) = match s.rsplit_once('/') {
            Some((prefix, last)) if !prefix.is_empty() => (Some(prefix.to_string()), last),
            _ => (None, s),
        };
        let (ty, slug) = node_part
            .split_once(':')
            .ok_or(IdParseError::MissingColon)?;
        if ty.is_empty() {
            return Err(IdParseError::EmptyType);
        }
        if slug.is_empty() {
            return Err(IdParseError::EmptySlug);
        }
        Ok(NodeId {
            node_type: NodeType::new(ty),
            slug: slug.to_string(),
            scope,
        })
    }
}

// Order by canonical string so "sorted by id ascending" matches what a reader sees.
impl Ord for NodeId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_string().cmp(&other.to_string())
    }
}

impl PartialOrd for NodeId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Serialize/Deserialize as the flat `type:slug` string so it round-trips through
// JSON output and SQLite columns unchanged.
impl Serialize for NodeId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdParseError {
    #[error("id is missing a ':' type prefix")]
    MissingColon,
    #[error("id has an empty type prefix")]
    EmptyType,
    #[error("id has an empty slug")]
    EmptySlug,
}
