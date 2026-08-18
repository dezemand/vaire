//! The adopted-changes digest (registry.md §3.2).
//!
//! Advancing a dependency raises one question — *what changed under me?* — and the honest
//! answer is almost never the publisher's changelog, which describes everything that
//! happened to a package most of whose entities a given consumer has never cited.
//!
//! Both halves of the better answer are already in the graph. A release record carries
//! `added` / `changed` / `retired` edges to the entities that release touched, and the
//! consuming package's own index carries edges to the entities it references. The digest is
//! their intersection: short by construction, and specific to this consumer.
//!
//! Two decisions worth naming:
//!
//! * **Frontmatter edges only.** A record also lists the same ids as `[[wikilinks]]` in its
//!   body, which are edges too; counting both would double every entity. The frontmatter
//!   keys are the structured claim, and their `ref_type` is what says *how* an entity was
//!   touched.
//! * **Nothing here can fail a pull.** The bytes arrived and are sound; a digest that could
//!   not be computed is a missing courtesy, not a failed acquisition. Every step degrades to
//!   "no digest" rather than to an error.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::index::Index;
use crate::model::Version;
use crate::output::{Adopted, AdoptedChange};

/// The edge types a release record uses to say what it touched.
///
/// `removed` is deliberately absent, and cannot be added: a removed entity has no address
/// left to point at, so the record names it in a code span rather than a reference (see
/// [`crate::release::record`]). It is the one change a consumer cannot be told about this
/// way — which is survivable, because removals only happen in a MAJOR, and a MAJOR is never
/// adopted silently.
const TOUCHED: [&str; 3] = ["added", "changed", "retired"];

/// What advancing `name` from `from` to `to` changed that `consumer` cites.
///
/// `entry` is the newly materialized store entry: it holds the release records for every
/// version in the range, because a record ships inside the release it describes.
pub fn digest(
    consumer: &Index,
    entry: &Path,
    name: &str,
    from: Version,
    to: Version,
) -> Option<Adopted> {
    // The dependency's own manifest names its release type — a package may call it anything
    // (`release_type` exists precisely so a corpus that already uses the word for something
    // else is not forced to rename), so guessing would silently find nothing.
    let config = crate::config::Config::load(&entry.join("knowledge.toml")).ok()?;
    let index = Index::open(&entry.join(".vaire").join("index.db")).ok()?;

    let records = records_between(&index, &config.release_type, from, to).ok()?;
    if records.is_empty() {
        return None;
    }
    let touched = touched_by(&index, &records).ok()?;
    if touched.is_empty() {
        return None;
    }
    let cited = cited_ids(consumer, name).ok()?;

    let mut changes: Vec<AdoptedChange> = touched
        .iter()
        .filter(|(id, _)| cited.contains(*id))
        .map(|(id, change)| AdoptedChange {
            id: id.clone(),
            change: change.clone(),
        })
        .collect();
    changes.sort_by(|a, b| a.id.cmp(&b.id));

    Some(Adopted {
        from: from.to_string(),
        to: to.to_string(),
        cited: changes,
        touched: touched.len(),
        unread: Vec::new(),
    })
}

/// The release records for every version in `(from, to]`.
///
/// Half-open at the bottom: `from` is the version being left behind, and what it changed was
/// adopted whenever *it* was pulled. Closed at the top, because `to` is what just arrived.
fn records_between(
    index: &Index,
    release_type: &str,
    from: Version,
    to: Version,
) -> crate::error::Result<Vec<String>> {
    let ids = index.query_rows(
        "SELECT id FROM nodes WHERE type = ?1",
        [release_type],
        |row| crate::db::col_text(row, 0),
    )?;
    Ok(ids
        .into_iter()
        .filter(|id| version_of(id).is_some_and(|version| version > from && version <= to))
        .collect())
}

/// Every entity the given records touched, and how.
///
/// A `BTreeMap` keyed by entity: one entity changed by two releases in the range is one
/// thing that changed, not two. The later record's verb wins — records are visited in
/// version order, and what a thing *became* is more useful than what it was on the way.
fn touched_by(index: &Index, records: &[String]) -> crate::error::Result<BTreeMap<String, String>> {
    let mut records = records.to_vec();
    records.sort_by_key(|id| version_of(id));

    let mut out = BTreeMap::new();
    for record in &records {
        let rows = index.query_rows(
            "SELECT to_id, ref_type FROM edges
              WHERE from_id = ?1 AND to_package IS NULL AND ref_type IN ('added','changed','retired')
              ORDER BY to_id",
            [record.as_str()],
            |row| {
                Ok((
                    crate::db::col_text(row, 0)?,
                    crate::db::col_text(row, 1)?,
                ))
            },
        )?;
        for (to_id, ref_type) in rows {
            debug_assert!(TOUCHED.contains(&ref_type.as_str()));
            out.insert(to_id, ref_type);
        }
    }
    Ok(out)
}

/// Every id in `name` that this package references.
fn cited_ids(consumer: &Index, name: &str) -> crate::error::Result<BTreeSet<String>> {
    Ok(consumer
        .query_rows(
            "SELECT DISTINCT to_id FROM edges WHERE to_package = ?1",
            [name],
            |row| crate::db::col_text(row, 0),
        )?
        .into_iter()
        .collect())
}

/// The version a release record's id names: `release:1-4-2` → `1.4.2`.
///
/// Dots are not in the id grammar, so the record's id carries the version with dashes (see
/// [`crate::release::record`]); this is that mapping read backwards. Anything that does not
/// have the shape is not a release record this can place in a range, and is skipped rather
/// than guessed at — a package's release type may well hold hand-written entities too.
fn version_of(id: &str) -> Option<Version> {
    let slug = id.rsplit_once(':').map_or(id, |(_, local)| local);
    let mut parts = slug.split('-');
    let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    format!("{major}.{minor}.{patch}").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_id_reads_back_as_the_version_it_names() {
        assert_eq!(version_of("release:1-4-2"), Some(Version::new(1, 4, 2)));
        assert_eq!(version_of("10-0-12"), Some(Version::new(10, 0, 12)));
        // A package's release type can hold entities that are not records; those simply do
        // not sit anywhere in a version range.
        assert_eq!(version_of("release:policy"), None);
        assert_eq!(version_of("release:1-4"), None);
        assert_eq!(version_of("release:1-4-2-rc1"), None);
    }
}
