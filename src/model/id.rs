//! Typed node IDs (design.md §6, §10).
//!
//! An ID is `<type>:<slug>` — e.g. `person:jane-doe`, `record:2026-06-10-broker-sync`.
//! The prefix is the **authoritative type** (the frontmatter `type:` field is only a
//! readable echo Vairë validates against). The vocabulary is *growable*, so the type
//! is an open string newtype, not a closed enum — a prefix Vairë has never seen is
//! still a valid node type (design.md §10, "Vocabulary will grow").
//!
//! Parsing has two deliberately different entry points:
//! - [`FromStr`] is the strict reference-**target** grammar (design.md §6): charset-
//!   validated so a target is identifiable by shape alone, without consulting config.
//! - [`NodeId::parse_stored`] is the lenient structural parse for IDs the index itself
//!   stored — a declared id is data (files are truth) and must round-trip unchanged.

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
/// `project:atlas-2026-q2/record:standup`). A **cross-package** reference additionally
/// carries a leading `@<package>/` qualifier (`@acme-core/department:platform`) — always
/// explicit, so resolution never depends on the consumer's dependency set (packages.md
/// §2). `node_type`/`slug` always describe the node itself; `scope` is the within-package
/// prefix; `package` is the owning package for a cross-package target (`None` = local).
/// See cli.md §6.1, design.md §6.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeId {
    pub node_type: NodeType,
    pub slug: String,
    pub scope: Option<String>,
    pub package: Option<String>,
}

impl NodeId {
    pub fn new(node_type: NodeType, slug: impl Into<String>) -> Self {
        NodeId {
            node_type,
            slug: slug.into(),
            scope: None,
            package: None,
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

    /// The owning package for a cross-package target (`@pkg/…`), or `None` when local.
    pub fn package(&self) -> Option<&str> {
        self.package.as_deref()
    }

    /// The node's own local slug (never includes the scope).
    pub fn local(&self) -> &str {
        &self.slug
    }

    /// The node's own ID without any scope prefix (`project:a/record:b` → `record:b`).
    pub fn local_id(&self) -> String {
        format!("{}:{}", self.node_type, self.slug)
    }

    /// The within-package address — scope path + node, **never** the `@package/` qualifier.
    /// This is what the index stores as an edge's `to_id`; the package travels separately
    /// in `to_package`.
    pub fn within_package(&self) -> String {
        match &self.scope {
            Some(scope) => format!("{}/{}:{}", scope, self.node_type, self.slug),
            None => format!("{}:{}", self.node_type, self.slug),
        }
    }

    /// Set (or replace) the scope prefix.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Set (or replace) the owning package (`@pkg/` qualifier).
    pub fn with_package(mut self, package: impl Into<String>) -> Self {
        self.package = Some(package.into());
        self
    }

    /// Parse an ID string the index itself stored (`nodes.id`, `edges.from_id`, …), or one
    /// reconstructed for display (`@pkg/scope/type:slug`).
    ///
    /// Deliberately **lenient** (structure only: a leading `@pkg/`, then last `/` splits
    /// the scope, first `:` splits `type:slug`) where [`FromStr`] is strict. A *declared*
    /// id is data — a file with `id: Jane_Doe` still indexes (files are truth) and must
    /// round-trip through the index unchanged. The strict grammar governs what a
    /// *reference* can say, not what the corpus may declare; `vaire check` surfaces the
    /// gap (`unreferenceable_id`).
    ///
    /// Panics on a string without a `:` — the index only ever stores composed
    /// `type:slug` ids, so that would be an internal invariant violation, not bad input.
    pub fn parse_stored(s: &str) -> NodeId {
        let (package, rest) = match s.strip_prefix('@').and_then(|a| a.split_once('/')) {
            Some((pkg, rest)) => (Some(pkg.to_string()), rest),
            None => (None, s),
        };
        let (scope, node_part) = match rest.rsplit_once('/') {
            Some((prefix, last)) if !prefix.is_empty() => (Some(prefix.to_string()), last),
            _ => (None, rest),
        };
        let (ty, slug) = node_part
            .split_once(':')
            .expect("stored ids are composed type:slug");
        NodeId {
            node_type: NodeType::new(ty),
            slug: slug.to_string(),
            scope,
            package,
        }
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(package) = &self.package {
            write!(f, "@{package}/")?;
        }
        match &self.scope {
            Some(scope) => write!(f, "{}/{}:{}", scope, self.node_type, self.slug),
            None => write!(f, "{}:{}", self.node_type, self.slug),
        }
    }
}

/// Parse a reference **target** against the strict grammar (design.md §6):
///
/// ```text
/// target  := [ "@" package "/" ] entity ( "/" entity )*   # >1 entity = scoped
/// entity  := type ":" id
/// type    := [a-z][a-z0-9-]*            # lowercase, starts with a letter
/// id      := [a-z0-9][a-z0-9-]*         # lowercase; no '.', no '/', no '@'
/// package := [a-z][a-z0-9-]*            # never contains ':' or '/'
/// ```
///
/// The charset is strict **on purpose**: identification is by shape alone, so a URL, an
/// email, a time, or a date is structurally not an ID and a colon in ordinary prose is
/// never mistaken for a reference (§6, identification vs classification). A leading
/// `@package/` marks a cross-package target; it is *recorded* now (packages.md §3),
/// *resolved* across a local workspace in M5.
impl FromStr for NodeId {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut segments: Vec<&str> = s.split('/').collect();

        // Optional leading `@package/` qualifier — decidable by shape: a first segment
        // starting with `@` is the package (it carries no `:`, unlike an entity).
        let package = match segments.first().and_then(|seg| seg.strip_prefix('@')) {
            Some(pkg) => {
                if !is_package(pkg) {
                    return Err(IdParseError::BadPackage);
                }
                let pkg = pkg.to_string();
                segments.remove(0);
                Some(pkg)
            }
            None => None,
        };

        let node_part = segments.pop().ok_or(IdParseError::MissingColon)?;
        if !segments.iter().all(|seg| is_entity(seg)) {
            return Err(IdParseError::BadScopeSegment);
        }
        let (ty, slug) = node_part
            .split_once(':')
            .ok_or(IdParseError::MissingColon)?;
        if ty.is_empty() {
            return Err(IdParseError::EmptyType);
        }
        if slug.is_empty() {
            return Err(IdParseError::EmptySlug);
        }
        if !is_type(ty) {
            return Err(IdParseError::BadType);
        }
        if !is_slug(slug) {
            return Err(IdParseError::BadSlug);
        }
        Ok(NodeId {
            node_type: NodeType::new(ty),
            slug: slug.to_string(),
            scope: if segments.is_empty() {
                None
            } else {
                Some(segments.join("/"))
            },
            package,
        })
    }
}

/// `type := [a-z][a-z0-9-]*` — lowercase, starts with a letter.
fn is_type(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
}

/// `package := [a-z][a-z0-9-]*` — same charset as a type (matches `config.rs::is_slug`).
fn is_package(s: &str) -> bool {
    is_type(s)
}

/// `id := [a-z0-9][a-z0-9-]*` — lowercase; no `.`, no `/`, no `@`, no uppercase.
fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
}

/// `entity := type ":" id` — one scope-path segment.
fn is_entity(seg: &str) -> bool {
    match seg.split_once(':') {
        Some((ty, id)) => is_type(ty) && is_slug(id),
        None => false,
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
    #[error("type must match [a-z][a-z0-9-]* (lowercase, starting with a letter)")]
    BadType,
    #[error("id must match [a-z0-9][a-z0-9-]* (lowercase; no '.', '/', or '@')")]
    BadSlug,
    #[error("scope segment is not a type:id entity")]
    BadScopeSegment,
    #[error("package must match [a-z][a-z0-9-]* (lowercase, starting with a letter)")]
    BadPackage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_grammar_accepts_conforming_targets() {
        for ok in [
            "department:platform",
            "person:jane-doe",
            "record:2026-06-10-broker-sync",
            "project:atlas-2026-q2/record:standup",
        ] {
            assert!(ok.parse::<NodeId>().is_ok(), "{ok} should parse");
        }
    }

    #[test]
    fn identification_is_by_shape_alone() {
        // design.md §6: URLs, emails, paths, times, versions, and dates are structurally
        // not references — the charset decides, no config consulted.
        for not_a_ref in [
            "https://somewhere",
            "mailto:a@b.com",
            "C:\\Users",
            "12:30",
            "1.2.3",
            "2026-06-15",
            "person:Jane",     // uppercase
            "person:jane_doe", // underscore
            "some_type:x",     // underscore in type
            "note:a.b",        // dot
        ] {
            assert!(
                not_a_ref.parse::<NodeId>().is_err(),
                "{not_a_ref} must not parse"
            );
        }
    }

    #[test]
    fn scope_segments_must_be_entities() {
        assert!("project:atlas/record:kickoff".parse::<NodeId>().is_ok());
        assert!("notanentity/record:kickoff".parse::<NodeId>().is_err());
    }

    #[test]
    fn cross_package_targets_parse_and_round_trip() {
        // packages.md §3: `@package/` marks a cross-package target — recorded here.
        let x: NodeId = "@acme-core/department:platform".parse().unwrap();
        assert_eq!(x.package(), Some("acme-core"));
        assert_eq!(x.scope(), None);
        assert_eq!(x.local_id(), "department:platform");
        assert_eq!(x.within_package(), "department:platform");
        assert_eq!(x.to_string(), "@acme-core/department:platform");

        // With a within-package scope path.
        let scoped: NodeId = "@acme-core/project:atlas/record:standup".parse().unwrap();
        assert_eq!(scoped.package(), Some("acme-core"));
        assert_eq!(scoped.scope(), Some("project:atlas"));
        assert_eq!(scoped.within_package(), "project:atlas/record:standup");
        assert_eq!(
            scoped.to_string(),
            "@acme-core/project:atlas/record:standup"
        );
    }

    #[test]
    fn cross_package_rejects_bad_package_and_stray_at() {
        assert!("@Acme-Core/department:x".parse::<NodeId>().is_err()); // uppercase package
        assert!("@acme-core".parse::<NodeId>().is_err()); // package, no entity
        assert!("person:jane@doe".parse::<NodeId>().is_err()); // @ mid-slug still fails
    }

    #[test]
    fn parse_stored_round_trips_nonconforming_declared_ids() {
        // Files are truth: a declared id outside the grammar still round-trips through
        // the index unchanged (check flags it as unreferenceable_id).
        let id = NodeId::parse_stored("person:Jane_Doe");
        assert_eq!(id.to_string(), "person:Jane_Doe");
        let scoped = NodeId::parse_stored("project:atlas/record:Kick.Off");
        assert_eq!(scoped.scope(), Some("project:atlas"));
        assert_eq!(scoped.to_string(), "project:atlas/record:Kick.Off");
    }

    #[test]
    fn parse_stored_reconstructs_cross_package_display() {
        let id = NodeId::parse_stored("@acme-core/project:atlas/record:x");
        assert_eq!(id.package(), Some("acme-core"));
        assert_eq!(id.scope(), Some("project:atlas"));
        assert_eq!(id.within_package(), "project:atlas/record:x");
        assert_eq!(id.to_string(), "@acme-core/project:atlas/record:x");
    }
}
