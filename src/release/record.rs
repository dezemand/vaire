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
//!
//! Prose may also arrive from outside ([`crate::release::summary`], `--summary <file>`).
//! It lands as a `## Summary` section and its frontmatter merges, but it never displaces
//! what the classifier computed — and the caller re-runs `check` over the written record,
//! so a reference somebody imagined cannot ship.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{Result, VaireError};
use crate::model::{Bump, Version};
use crate::release::classify::Classification;
use crate::release::summary::Summary;

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
    summary: Option<&Summary>,
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
    std::fs::write(
        &path,
        render(config, version, bump, classification, notes, summary)?,
    )?;
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
    summary: Option<&Summary>,
) -> Result<String> {
    let date = crate::clock::today_utc();
    // The author's title when there is one — the version is the identity regardless, since
    // it is the `id`, and the headline below names it whenever the title does not.
    let title = summary
        .and_then(Summary::name)
        .map(str::to_string)
        .unwrap_or_else(|| version.to_string());

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", slug(version)));
    out.push_str(&format!("type: {}\n", config.release_type));
    out.push_str(&format!("name: {}\n", yaml_string(&title)));
    out.push_str(&format!("aliases: [{}]\n", aliases(version, summary)));
    out.push_str(&format!("date: {date}\n"));
    if let Some(bump) = bump {
        out.push_str(&format!("bump: {bump}\n"));
    }
    if summary.is_some() {
        // What Vairë can honestly assert: this prose did not come from the classifier. Who
        // or what wrote it is the author's to declare, and rides along in the free keys.
        out.push_str("generated_summary: true\n");
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
    if let Some(summary) = summary {
        out.push_str(&summary.free_frontmatter()?);
    }
    out.push_str("---\n");

    out.push_str(&format!("# {title}\n\n"));
    // A retitled record must still say which version it is, in prose a reader sees.
    let named_version = (title != version.to_string()).then(|| format!("{version} — "));
    let headline = match bump {
        Some(bump) => format!(
            "Released {date} — {}{bump} — {}.",
            named_version.unwrap_or_default(),
            classification.evidence()
        ),
        None => format!(
            "Released {date} — {}the first published version.",
            named_version.unwrap_or_default()
        ),
    };
    out.push_str(&headline);
    out.push('\n');

    if let Some(summary) = summary {
        out.push_str("\n## Summary\n\n");
        out.push_str(summary.prose.trim());
        out.push('\n');
    }

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
    Ok(out)
}

/// The record's alias list: the two version spellings first, then whatever the summary
/// added. Union rather than replacement — the spellings are how a record is found, so an
/// author may extend them but never displace them.
fn aliases(version: Version, summary: Option<&Summary>) -> String {
    let mut aliases = vec![version.to_string(), format!("v{version}")];
    for extra in summary.map(Summary::aliases).unwrap_or_default() {
        if !aliases.contains(&extra) {
            aliases.push(extra);
        }
    }
    aliases
        .iter()
        .map(|a| yaml_string(a))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A double-quoted YAML scalar. Titles are author-supplied text that routinely contains
/// the characters YAML reads as syntax — a colon in `Workshop D: Agents` would otherwise
/// turn one key into two and stop the file being a node at all.
fn yaml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        // A line break inside a one-line scalar would end the key mid-value.
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_slug_drops_the_dots_the_id_grammar_forbids() {
        assert_eq!(slug(Version::new(1, 7, 0)), "1-7-0");
        assert_eq!(slug(Version::new(10, 0, 12)), "10-0-12");
    }

    fn classification() -> Classification {
        let mut c = crate::release::classify::initial();
        c.added = vec!["system:gateway".to_string()];
        c
    }

    fn rendered(summary: Option<&Summary>) -> String {
        render(
            &Config::default(),
            Version::new(1, 5, 0),
            Some(Bump::Minor),
            &classification(),
            None,
            summary,
        )
        .expect("render")
    }

    #[test]
    fn without_a_summary_the_record_is_what_it_always_was() {
        let out = rendered(None);
        assert!(out.contains("name: \"1.5.0\"\n"), "{out}");
        assert!(out.contains("aliases: [\"1.5.0\", \"v1.5.0\"]\n"), "{out}");
        assert!(out.contains("\n# 1.5.0\n"), "{out}");
        assert!(!out.contains("generated_summary"), "{out}");
        assert!(!out.contains("## Summary"), "{out}");
    }

    #[test]
    fn a_summary_lands_as_a_section_and_is_marked_as_generated() {
        let summary = crate::release::summary::parse(
            "Three systems joined, and [[system:gateway]] absorbed the routing rules.\n",
            "summary.md",
        )
        .unwrap();
        let out = rendered(Some(&summary));
        assert!(out.contains("generated_summary: true\n"), "{out}");
        let summary_at = out.find("## Summary").expect("summary section");
        let added_at = out.find("## Added").expect("added section");
        // Overview first, the computed lists after it.
        assert!(summary_at < added_at, "{out}");
        assert!(out.contains("[[system:gateway]] absorbed"), "{out}");
    }

    #[test]
    fn an_author_title_wins_the_name_and_the_headline_keeps_the_version() {
        let summary = crate::release::summary::parse(
            "---\nname: \"Gateway consolidation\"\naliases: [\"gateway-release\"]\n\
             summary_by: an agent\n---\nProse.\n",
            "summary.md",
        )
        .unwrap();
        let out = rendered(Some(&summary));
        assert!(out.contains("name: \"Gateway consolidation\"\n"), "{out}");
        // The computed spellings survive the author's additions.
        assert!(
            out.contains("aliases: [\"1.5.0\", \"v1.5.0\", \"gateway-release\"]\n"),
            "{out}"
        );
        assert!(out.contains("\n# Gateway consolidation\n"), "{out}");
        // A retitled record still says which version it is, in prose.
        assert!(out.contains("Released "), "{out}");
        assert!(out.contains("1.5.0 — minor"), "{out}");
        // Free keys ride along.
        assert!(out.contains("summary_by: an agent\n"), "{out}");
        // …and the id is untouched by any of it.
        assert!(out.contains("id: 1-5-0\n"), "{out}");
    }

    #[test]
    fn an_author_title_is_quoted_so_a_colon_cannot_split_the_key() {
        let summary =
            crate::release::summary::parse("---\nname: \"Workshop D: Agents\"\n---\nP.\n", "s.md")
                .unwrap();
        let out = rendered(Some(&summary));
        assert!(out.contains("name: \"Workshop D: Agents\"\n"), "{out}");
        // The rendered record must still parse as a node.
        let doc = crate::corpus::frontmatter::split(&out).expect("record is frontmatter-parseable");
        assert_eq!(
            doc.frontmatter.get("name").and_then(|v| v.as_str()),
            Some("Workshop D: Agents")
        );
    }
}
