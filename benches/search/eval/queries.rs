//! Query-file loading and validation (`benches/search/README.md` documents the format).
//!
//! A query file is a TOML array of `[[query]]` tables, each with a unique `id`, the
//! `text` to search for, a free-form `category` label (`name | alias | keyword |
//! question | paraphrase | document | filter` are the documented ones; the synthetic
//! `scale` corpus also uses `latency` for judgment-less queries), an optional `type`
//! (→ `SearchOpts.type_filter`), an optional `note`, and an optional `[query.relevant]`
//! table mapping judged node ids to a relevance grade (`2` = primary answer, `1` = also
//! relevant). A query with no `[query.relevant]` table carries no judgments and is
//! excluded from quality metrics (but still measured for latency).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One query loaded from a `queries.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub id: String,
    pub text: String,
    pub category: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Judged node ids → grade (`1` or `2`). Empty ⇒ this query carries no judgments
    /// (excluded from quality metrics, still included in latency measurement).
    #[serde(default)]
    pub relevant: BTreeMap<String, u8>,
}

impl Query {
    /// Whether this query carries any relevance judgments at all — the gate for
    /// inclusion in quality metrics (`benches/search/README.md`).
    pub fn is_judged(&self) -> bool {
        !self.relevant.is_empty()
    }

    /// The judged id with grade `2` (the primary answer), if any; [`parse`] allows at most
    /// one per query.
    pub fn primary_expected_id(&self) -> Option<&str> {
        self.relevant
            .iter()
            .find(|&(_, &grade)| grade == 2)
            .map(|(id, _)| id.as_str())
    }
}

#[derive(Debug, Default, Deserialize)]
struct QueryFile {
    #[serde(default)]
    query: Vec<Query>,
}

/// Load and validate a query file from disk.
pub fn load(path: &Path) -> Result<Vec<Query>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    parse(&text, &path.display().to_string())
}

/// Parse and validate query-file text. `origin` labels errors (a path for [`load`]).
///
/// Validates: every `id` non-empty and unique, every judgment grade in `1..=2`, and at
/// most one grade-`2` (primary) judgment per query. Existence of judged ids in the index
/// can only be checked after indexing — see [`warn_missing_ids`].
pub fn parse(text: &str, origin: &str) -> Result<Vec<Query>, String> {
    let file: QueryFile =
        toml::from_str(text).map_err(|e| format!("{origin}: invalid queries.toml: {e}"))?;
    let mut seen = HashSet::with_capacity(file.query.len());
    for q in &file.query {
        if q.id.trim().is_empty() {
            return Err(format!("{origin}: a query has an empty id"));
        }
        if !seen.insert(q.id.as_str()) {
            return Err(format!("{origin}: duplicate query id {:?}", q.id));
        }
        for (judged_id, grade) in &q.relevant {
            if !(1..=2).contains(grade) {
                return Err(format!(
                    "{origin}: query {:?} judges {judged_id:?} with grade {grade} \
                     (must be 1 or 2)",
                    q.id
                ));
            }
        }
        let primaries: Vec<&str> = q
            .relevant
            .iter()
            .filter(|&(_, &grade)| grade == 2)
            .map(|(id, _)| id.as_str())
            .collect();
        if primaries.len() > 1 {
            return Err(format!(
                "{origin}: query {:?} grades {} ids as 2 ({}); only one primary answer is allowed",
                q.id,
                primaries.len(),
                primaries.join(", ")
            ));
        }
    }
    Ok(file.query)
}

/// Every judged id across `queries` that the freshly-built index does not actually
/// contain, sorted — a stale or typoed judgment. The caller is expected to warn loudly
/// (this harness prints it and records it as a report notice); it is deliberately not an
/// error, since "real thresholds get added later" per the issue #52 harness spec.
pub fn warn_missing_ids(index: &vaire::index::db::Index, queries: &[Query]) -> Vec<String> {
    let existing: HashSet<String> = index
        .query_rows("SELECT id FROM nodes", (), |r| {
            vaire::index::db::col_text(r, 0)
        })
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut missing: Vec<String> = queries
        .iter()
        .flat_map(|q| q.relevant.keys())
        .filter(|id| !existing.contains(id.as_str()))
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

// See the comment on the `tests` module in `corpus.rs`: this file also compiles into the
// `harness = false` search bench, where these test-only items are unreferenced.
#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_shape() {
        let toml = r#"
[[query]]
id = "name-loose-end"
text = "loose end"
category = "name"
type = "concept"
note = "optional"
[query.relevant]
"concept:loose-end" = 2
"skill:vaire-files" = 1
"#;
        let qs = parse(toml, "test").unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].id, "name-loose-end");
        assert_eq!(qs[0].type_filter.as_deref(), Some("concept"));
        assert_eq!(qs[0].relevant.get("concept:loose-end"), Some(&2));
        assert!(qs[0].is_judged());
        assert_eq!(qs[0].primary_expected_id(), Some("concept:loose-end"));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let toml = r#"
[[query]]
id = "dup"
text = "a"
category = "keyword"
[[query]]
id = "dup"
text = "b"
category = "keyword"
"#;
        assert!(parse(toml, "test").unwrap_err().contains("duplicate"));
    }

    #[test]
    fn rejects_out_of_range_grades() {
        let toml = r#"
[[query]]
id = "q"
text = "a"
category = "keyword"
[query.relevant]
"concept:x" = 3
"#;
        assert!(parse(toml, "test").unwrap_err().contains("must be 1 or 2"));
    }

    #[test]
    fn rejects_more_than_one_primary_judgment() {
        let toml = r#"
[[query]]
id = "q"
text = "a"
category = "keyword"
[query.relevant]
"concept:x" = 2
"concept:y" = 2
"concept:z" = 1
"#;
        let err = parse(toml, "test").unwrap_err();
        assert!(err.contains("only one primary answer"), "{err}");
        assert!(err.contains("concept:x, concept:y"), "{err}");
    }

    #[test]
    fn query_without_relevant_table_is_unjudged() {
        let toml = r#"
[[query]]
id = "latency-only"
text = "a b c"
category = "latency"
"#;
        let qs = parse(toml, "test").unwrap();
        assert!(!qs[0].is_judged());
        assert_eq!(qs[0].primary_expected_id(), None);
    }
}
