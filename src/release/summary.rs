//! `--summary <file>` — prose somebody else wrote, placed in the release record.
//!
//! The seam between `vaire release` and a changelog-writing agent is **a file**, never an
//! API. Vairë does not talk to a model, does not hold a prompt, and does not know what
//! produced the bytes it is handed: it reads Markdown, merges any frontmatter the author
//! attached, and lets `check` decide whether the result is admissible corpus. That keeps
//! the whole feature opt-in and keeps an LLM out of the tool — a package must remain
//! releasable by someone who has no model at all.
//!
//! What the file may say is bounded by one rule: **the record's claims about the release
//! stay computed.** The version, the bump and the three entity lists come from the
//! classifier; a summary that sets one of them is refused by name rather than ignored, so
//! an author never believes they overrode something they did not. Everything else merges —
//! `aliases` by union, so the version spellings always survive, and any other key
//! verbatim.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Result, VaireError};

/// Frontmatter the record computes for itself. A summary naming one of these is refused:
/// they are the release's claims about what it did, and the point of a computed version is
/// that nobody — human or agent — types it.
pub const COMPUTED_KEYS: &[&str] = &[
    "id",
    "type",
    "date",
    "bump",
    "added",
    "changed",
    "retired",
    "generated_summary",
];

/// Frontmatter merged rather than replaced. `aliases` is the one: the dotted and `v`-prefixed
/// version spellings are how a record is *found*, so an author may add to them but not
/// displace them.
pub const MERGED_KEYS: &[&str] = &["aliases"];

/// A parsed summary file: the prose, and whatever frontmatter rode along with it.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// The Markdown below the frontmatter (or the whole file, when there was none).
    pub prose: String,
    pub frontmatter: BTreeMap<String, serde_yaml::Value>,
}

impl Summary {
    /// The title the record should carry, when the author chose one. A release named
    /// "Gateway consolidation" reads better than one named "1.5.0", and the version is the
    /// identity either way — it is the `id`, and it stays in the headline.
    pub fn name(&self) -> Option<&str> {
        self.frontmatter.get("name").and_then(|v| v.as_str())
    }

    /// Extra aliases to union into the computed version spellings. Non-string entries are
    /// dropped rather than refused — an alias list is a display convenience, and half a
    /// list is better than a failed release.
    pub fn aliases(&self) -> Vec<String> {
        match self.frontmatter.get("aliases") {
            Some(serde_yaml::Value::Sequence(items)) => items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect(),
            Some(serde_yaml::Value::String(one)) => vec![one.clone()],
            _ => Vec::new(),
        }
    }

    /// The keys the record does not write itself, rendered as a YAML block ready to append
    /// to the frontmatter. Sorted (a `BTreeMap`), so the same summary always produces the
    /// same bytes.
    pub fn free_frontmatter(&self) -> Result<String> {
        let mut free = serde_yaml::Mapping::new();
        for (key, value) in &self.frontmatter {
            if COMPUTED_KEYS.contains(&key.as_str())
                || MERGED_KEYS.contains(&key.as_str())
                || key == "name"
            {
                continue;
            }
            free.insert(serde_yaml::Value::String(key.clone()), value.clone());
        }
        if free.is_empty() {
            return Ok(String::new());
        }
        serde_yaml::to_string(&serde_yaml::Value::Mapping(free))
            .map_err(|e| VaireError::Release(format!("summary frontmatter is not writable: {e}")))
    }
}

/// Read and validate a summary file.
pub fn read(path: &Path) -> Result<Summary> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| VaireError::Release(format!("{}: {e}", path.display())))?;
    parse(&text, &path.display().to_string())
}

/// [`read`] against text already in hand (the seam the tests drive).
pub fn parse(text: &str, origin: &str) -> Result<Summary> {
    let summary = match crate::corpus::frontmatter::split(text) {
        Some(doc) => Summary {
            prose: doc.prose,
            frontmatter: doc.frontmatter,
        },
        // No frontmatter block: the whole file is prose. But a file that *opens* with a
        // fence and still did not split has a malformed block — an unterminated fence or
        // YAML that is not a mapping — and silently demoting it to prose would write the
        // author's intended frontmatter into the record as body text.
        None => {
            if text.trim_start_matches('\u{feff}').starts_with("---") {
                return Err(VaireError::Release(format!(
                    "{origin}: the leading `---` block is not valid frontmatter — it needs a \
                     closing `---` fence and must parse as a YAML mapping"
                )));
            }
            Summary {
                prose: text.to_string(),
                frontmatter: BTreeMap::new(),
            }
        }
    };

    if summary.prose.trim().is_empty() {
        return Err(VaireError::Release(format!(
            "{origin} has no prose — a summary file carries the release notes themselves, \
             not only frontmatter"
        )));
    }
    // Refused by name, never silently dropped: an author who set `added:` believes they
    // described the release, and a record that quietly disagreed with its own edges would
    // be worse than a failed release.
    let taken: Vec<&str> = COMPUTED_KEYS
        .iter()
        .copied()
        .filter(|key| summary.frontmatter.contains_key(*key))
        .collect();
    if !taken.is_empty() {
        return Err(VaireError::Release(format!(
            "{origin} sets {} — the release computes {} from the classifier, so a summary \
             cannot override {}. Drop the key; `name`, `aliases` and any key of your own \
             are yours to set",
            taken.join(", "),
            match taken.len() {
                1 => "that key",
                _ => "those keys",
            },
            match taken.len() {
                1 => "it",
                _ => "them",
            },
        )));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_markdown_file_is_all_prose() {
        let s = parse("Three systems joined the graph.\n", "summary.md").unwrap();
        assert_eq!(s.prose, "Three systems joined the graph.\n");
        assert!(s.frontmatter.is_empty());
        assert_eq!(s.name(), None);
        assert_eq!(s.free_frontmatter().unwrap(), "");
    }

    #[test]
    fn frontmatter_splits_off_and_free_keys_round_trip_sorted() {
        let s = parse(
            "---\nname: Gateway consolidation\nsummary_by: an agent\naliases: [\"gateway\"]\n\
             theme: platform\n---\nProse.\n",
            "summary.md",
        )
        .unwrap();
        assert_eq!(s.prose.trim(), "Prose.");
        assert_eq!(s.name(), Some("Gateway consolidation"));
        assert_eq!(s.aliases(), vec!["gateway".to_string()]);
        // `name`/`aliases` are handled by the record itself; only the rest is appended,
        // sorted, and without a document-start marker.
        let free = s.free_frontmatter().unwrap();
        assert_eq!(free, "summary_by: an agent\ntheme: platform\n");
    }

    #[test]
    fn a_summary_may_not_claim_what_the_classifier_computes() {
        for key in COMPUTED_KEYS {
            let text = format!("---\n{key}: whatever\n---\nProse.\n");
            let err = parse(&text, "summary.md").unwrap_err().to_string();
            assert!(err.contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn a_malformed_frontmatter_block_is_an_error_not_prose() {
        // Opens a fence, never closes it: the author meant frontmatter.
        let err = parse("---\nname: Thing\nProse.\n", "summary.md")
            .unwrap_err()
            .to_string();
        assert!(err.contains("closing `---`"), "{err}");
    }

    #[test]
    fn frontmatter_without_prose_is_refused() {
        let err = parse("---\nname: Thing\n---\n\n", "summary.md")
            .unwrap_err()
            .to_string();
        assert!(err.contains("no prose"), "{err}");
    }
}
