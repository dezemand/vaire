//! Release records — the changelog, written as corpus.
//!
//! Every release writes an entity describing itself: the version, the date, the computed
//! bump, and **edges to the entities involved**. That last part is the point. A changelog
//! file would say the same things in prose; a record says them in the graph, so
//! "which releases touched this entity?" is an ordinary backlinks query, and a consumer
//! advancing a dependency can compute what changed *that it actually cites* by
//! intersecting these edges with its own.
//!
//! Three asymmetries are deliberate:
//!
//! * **Added, changed and retired become references; removed does not.** A removed entity
//!   has no address left to point at. It is recorded as text, which is honest, and costs
//!   nothing: removals only happen in a MAJOR, where a human reads the notes anyway.
//! * **MINOR and PATCH are auto-drafted; MAJOR is not.** The entity lists *are* "what
//!   knowledge arrived"; a MAJOR additionally carries invalidated assumptions that only a
//!   maintainer can write.
//! * **The record is written before the release commit**, so it ships inside the release
//!   it describes rather than trailing one commit behind it.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{Result, VaireError};
use crate::model::{Bump, Version};
use crate::release::classify::Classification;

/// A written record: where it landed, and the address it now answers to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Record {
    /// Package-root-relative, forward slashes — the form git and the index both want.
    pub path: String,
    pub id: String,
}

/// Render and write the record for `version`. Returns `None` when the package has opted
/// out of records entirely.
pub fn write(
    root: &Path,
    config: &Config,
    version: Version,
    bump: Option<Bump>,
    classification: &Classification,
    notes: Option<&str>,
) -> Result<Record> {
    let rel = format!(
        "{}/{}.md",
        config.release_dir.trim_end_matches('/'),
        slug(version)
    );
    let path = root.join(&rel);
    if path.exists() {
        return Err(VaireError::Release(format!(
            "{rel} already exists — version {version} has been released before, and a \
             published version is immutable"
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, render(config, version, bump, classification, notes))?;
    Ok(Record {
        path: rel,
        id: format!("{}:{}", config.release_type, slug(version)),
    })
}

/// `1.7.0` → `1-7-0`. Dots are not part of the id grammar, so the dotted form rides
/// along as an alias and both spellings resolve.
fn slug(version: Version) -> String {
    format!("{}-{}-{}", version.major, version.minor, version.patch)
}

fn render(
    config: &Config,
    version: Version,
    bump: Option<Bump>,
    classification: &Classification,
    notes: Option<&str>,
) -> String {
    let date = today();
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", slug(version)));
    out.push_str(&format!("type: {}\n", config.release_type));
    out.push_str(&format!("name: \"{version}\"\n"));
    out.push_str(&format!("aliases: [\"{version}\", \"v{version}\"]\n"));
    out.push_str(&format!("date: {date}\n"));
    if let Some(bump) = bump {
        out.push_str(&format!("bump: {bump}\n"));
    }
    for (key, ids) in [
        ("added", &classification.added),
        ("changed", &classification.changed),
        ("retired", &classification.retired),
    ] {
        if !ids.is_empty() {
            out.push_str(&format!("{key}: [{}]\n", ids.join(", ")));
        }
    }
    out.push_str("---\n");

    out.push_str(&format!("# {version}\n\n"));
    let headline = match bump {
        Some(bump) => format!("Released {date} — {bump} — {}.", classification.evidence()),
        None => format!("Released {date} — the first published version."),
    };
    out.push_str(&headline);
    out.push('\n');

    if let Some(notes) = notes {
        out.push_str("\n## Invalidated assumptions\n\n");
        out.push_str(notes.trim_end());
        out.push('\n');
    }

    for (heading, ids) in [
        ("Added", &classification.added),
        ("Changed", &classification.changed),
        ("Retired", &classification.retired),
    ] {
        if ids.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {heading}\n\n"));
        for id in ids {
            out.push_str(&format!("- [[{id}]]\n"));
        }
    }
    if !classification.removed.is_empty() {
        // Code spans, not references: these addresses no longer resolve, and the scanner
        // skips code so a record can name them without manufacturing dangling edges.
        out.push_str("\n## Removed\n\n");
        for id in &classification.removed {
            out.push_str(&format!("- `{id}`\n"));
        }
    }
    out
}

/// Where a record for `version` would land — for the preflight that refuses to write one
/// the package's own include globs would never index.
pub fn path_for(config: &Config, version: Version) -> PathBuf {
    PathBuf::from(format!(
        "{}/{}.md",
        config.release_dir.trim_end_matches('/'),
        slug(version)
    ))
}

/// Today in UTC as `YYYY-MM-DD`.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the Unix epoch → civil `(year, month, day)`. Howard Hinnant's `chrono`
/// algorithm, inlined rather than taking a date dependency for one format call.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = era * 400 + yoe + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // a leap year's start
        assert_eq!(civil_from_days(19_783), (2024, 3, 1)); // just past leap day
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn a_version_slug_drops_the_dots_the_id_grammar_forbids() {
        assert_eq!(slug(Version::new(1, 7, 0)), "1-7-0");
        assert_eq!(slug(Version::new(10, 0, 12)), "10-0-12");
    }
}
