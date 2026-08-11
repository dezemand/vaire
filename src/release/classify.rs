//! The release classifier — deriving the version bump from what changed
//! (registry.v2.md §3.1).
//!
//! The version stops being something a maintainer types. Because the index is a
//! *semantic* model of the package rather than a pile of text, the diff between the last
//! release and the current tree answers the question mechanically (the named precedent is
//! `elm bump`, which derives a version from an API diff):
//!
//! | observed                                                   | bump    |
//! |------------------------------------------------------------|---------|
//! | entity addresses added; none removed or retired             | MINOR   |
//! | content changed, address set identical                      | PATCH   |
//! | entity addresses removed, or a `superseded_by:` appeared    | MAJOR   |
//! | mixed                                                       | highest |
//!
//! **MAJOR is a claim about meaning, so it is never taken automatically** — the caller
//! gates it behind an explicit `--major`. The inverse also holds: `--major` may escalate
//! a textually tiny edit that reverses a truth, because the maintainer owns meaning while
//! this module only sees structure.
//!
//! A rename needs no special detection: an address *is* the identity, so renaming
//! `concept:foo` to `concept:bar` presents as one removal and one addition, and the
//! removal alone already forces MAJOR.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::Result;
use crate::index::Index;
use crate::index::db::{col_opt_text, col_text};
use crate::model::Bump;

/// What a release would be, and the evidence for it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Classification {
    pub outcome: Outcome,
    /// Addresses present now and absent at the baseline.
    pub added: Vec<String>,
    /// Addresses whose content, edges, or aliases differ from the baseline.
    pub changed: Vec<String>,
    /// Addresses that gained a `superseded_by:` — retired, but still resolvable.
    pub retired: Vec<String>,
    /// Addresses gone outright. Recorded as text rather than as references: a deleted
    /// entity has no address left to point at (and deleting instead of tombstoning is
    /// what the versioning rules ask maintainers not to do).
    pub removed: Vec<String>,
}

/// The classifier's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "bump")]
pub enum Outcome {
    /// No prior release tag: nothing to diff against, so the manifest's current version
    /// publishes verbatim. A first release is a declaration, not an increment.
    Initial,
    /// A baseline exists and the corpus is identical to it.
    Nothing,
    /// The computed bump.
    Bump(Bump),
}

impl Classification {
    /// The bump, or `None` when there is nothing to release (or nothing to compute
    /// because this is a first release).
    pub fn bump(&self) -> Option<Bump> {
        match self.outcome {
            Outcome::Bump(bump) => Some(bump),
            Outcome::Initial | Outcome::Nothing => None,
        }
    }

    /// Whether the classification carries a MAJOR signal, which the caller must gate
    /// behind an explicit opt-in.
    pub fn is_major(&self) -> bool {
        self.outcome == Outcome::Bump(Bump::Major)
    }

    /// A one-line human summary of the evidence ("3 new entities, 12 changed").
    pub fn evidence(&self) -> String {
        let mut parts = Vec::new();
        for (n, label) in [
            (self.added.len(), "new"),
            (self.changed.len(), "changed"),
            (self.retired.len(), "retired"),
            (self.removed.len(), "removed"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {label}"));
            }
        }
        match parts.is_empty() {
            true => "no entity changes".to_string(),
            false => parts.join(", "),
        }
    }
}

/// Everything about one entity that a consumer could notice.
///
/// Deliberately three things and not everything the index holds: what it can be found by
/// (aliases), what it says (sections), and what it points at (edges). A file that merely
/// moved, or whose `updated:` was touched, has not changed for anyone reading it — and
/// counting either as a change would make the classifier cry patch at bookkeeping.
#[derive(PartialEq, Eq)]
struct EntityState {
    superseded_by: Option<String>,
    aliases: String,
    /// Sorted, so re-ordering sections is not a content change; a multiset, so two
    /// identical sections stay two.
    sections: Vec<(String, String)>,
    edges: BTreeSet<(String, String, String)>,
}

/// Diff two indexes of the same package. `release_type` names the entity type holding
/// release records, which is **excluded from both sides**: every release adds one, so
/// counting them would mean no release after the first could ever be a PATCH — the
/// mechanism would eat itself.
pub fn diff(baseline: &Index, current: &Index, release_type: &str) -> Result<Classification> {
    let before = read_entities(baseline, release_type)?;
    let after = read_entities(current, release_type)?;

    let mut added = Vec::new();
    let mut changed = Vec::new();
    let mut retired = Vec::new();
    let mut removed = Vec::new();

    for (id, now) in &after {
        match before.get(id) {
            None => added.push(id.clone()),
            Some(then) if then == now => {}
            Some(then) => {
                // A tombstone is the retirement, not an edit: report it as the stronger
                // fact rather than as both.
                if then.superseded_by.is_none() && now.superseded_by.is_some() {
                    retired.push(id.clone());
                } else {
                    changed.push(id.clone());
                }
            }
        }
    }
    for id in before.keys() {
        if !after.contains_key(id) {
            removed.push(id.clone());
        }
    }

    // `max()` over the applicable severities is the "highest applicable" rule for a mixed
    // diff, expressed once.
    let outcome = [
        (!removed.is_empty() || !retired.is_empty()).then_some(Bump::Major),
        (!added.is_empty()).then_some(Bump::Minor),
        (!changed.is_empty()).then_some(Bump::Patch),
    ]
    .into_iter()
    .flatten()
    .max()
    .map_or(Outcome::Nothing, Outcome::Bump);

    Ok(Classification {
        outcome,
        added,
        changed,
        retired,
        removed,
    })
}

/// The classification of a package that has never been released.
pub fn initial() -> Classification {
    empty(Outcome::Initial)
}

/// The classification of a package with nothing new since its last release — the answer
/// `status` reaches without diffing anything, when no commit has landed since the tag.
pub fn nothing() -> Classification {
    empty(Outcome::Nothing)
}

fn empty(outcome: Outcome) -> Classification {
    Classification {
        outcome,
        added: Vec::new(),
        changed: Vec::new(),
        retired: Vec::new(),
        removed: Vec::new(),
    }
}

fn read_entities(index: &Index, release_type: &str) -> Result<BTreeMap<String, EntityState>> {
    let mut entities: BTreeMap<String, EntityState> = index
        .query_rows(
            "SELECT id, superseded_by, alias_text FROM nodes WHERE type <> ?1 ORDER BY id",
            [release_type],
            |row| {
                Ok((
                    col_text(row, 0)?,
                    EntityState {
                        superseded_by: col_opt_text(row, 1)?,
                        aliases: col_text(row, 2)?,
                        sections: Vec::new(),
                        edges: BTreeSet::new(),
                    },
                ))
            },
        )?
        .into_iter()
        .collect();

    for (node_id, heading, body) in index.query_rows(
        "SELECT s.node_id, s.heading, s.body
           FROM sections s JOIN nodes n ON n.id = s.node_id
          WHERE n.type <> ?1",
        [release_type],
        |row| Ok((col_text(row, 0)?, col_text(row, 1)?, col_text(row, 2)?)),
    )? {
        if let Some(entity) = entities.get_mut(&node_id) {
            entity.sections.push((heading, body));
        }
    }

    for (from_id, to) in index.query_rows(
        "SELECT e.from_id, e.to_id, e.to_package, e.ref_type
           FROM edges e JOIN nodes n ON n.id = e.from_id
          WHERE n.type <> ?1",
        [release_type],
        |row| {
            Ok((
                col_text(row, 0)?,
                (
                    col_text(row, 1)?,
                    col_opt_text(row, 2)?.unwrap_or_default(),
                    col_text(row, 3)?,
                ),
            ))
        },
    )? {
        if let Some(entity) = entities.get_mut(&from_id) {
            entity.edges.insert(to);
        }
    }

    // Row order from the joins above is not guaranteed, and a section multiset must
    // compare equal regardless of how the engine returned it.
    for entity in entities.values_mut() {
        entity.sections.sort();
    }
    Ok(entities)
}
