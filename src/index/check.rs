//! Integrity guards — `vaire check` (cli.md §4.2, design.md §9).
//!
//! ID-based discovery makes two checks fall out for free (unique IDs, resolvable refs)
//! plus two structural ones (drift, orphans). Read-only; exits `6` on any violation
//! (or any warning under `--strict`).

use crate::error::Result;
use crate::index::db::{Index, col_text, col_u32};
use crate::model::id::{NodeId, NodeType};

/// A hard violation (fails the check).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Violation {
    /// Two nodes sharing one `id:` — the duplicate-entity guard.
    DuplicateId { id: String, paths: Vec<String> },
    /// A non-`?` reference whose target ID is not a node.
    DanglingRef {
        from: String,
        to: String,
        path: String,
        line: u32,
    },
}

impl Violation {
    /// The stable `kind` string (matches the JSON tag).
    pub fn kind(&self) -> &'static str {
        match self {
            Violation::DuplicateId { .. } => "duplicate_id",
            Violation::DanglingRef { .. } => "dangling_ref",
        }
    }

    /// A one-line human description (no `kind` prefix).
    pub fn detail(&self) -> String {
        match self {
            Violation::DuplicateId { id, paths } => format!("{id}  (in {} files)", paths.len()),
            Violation::DanglingRef {
                from,
                to,
                path,
                line,
            } => {
                format!("{from} → {to}  {path}:{line}")
            }
        }
    }
}

/// A soft warning (fails only under `--strict`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Warning {
    /// A node with no inbound or outbound edges.
    Orphan { id: String, path: String },
    /// A resolved **inline** reference whose target is not also in the frontmatter
    /// edge list (cli.md §4.2). Advisory: the spec's own examples carry narrative inline
    /// links beyond the structured edge list, so this is a warning, not a failure — the
    /// actionable direction only ("you linked it in prose; declare it in frontmatter").
    Drift {
        id: String,
        to: String,
        path: String,
        line: u32,
    },
    /// A frontmatter value written with inline-style `[[ ]]` brackets (cli.md §6.3) — the
    /// muscle-memory trap. Frontmatter references are bare (`field: type:id` or
    /// `field: "?type: descriptor"`); brackets either no-op silently or, unquoted, parse
    /// to junk. Flagged so the mistake surfaces.
    FrontmatterWikilink {
        id: String,
        field: String,
        path: String,
    },
    /// A frontmatter value matching the reference target grammar (`field: team:alpha`)
    /// whose **type is not in `types`**, so it was *ignored* rather than made an edge
    /// (classification, design.md §6). Surfaces the silent drop — declare the type, or
    /// quote the value as a string. Values that fail identification (URLs, times, prose
    /// with a colon) are plain scalars and never flagged.
    UnknownType {
        id: String,
        field: String,
        value: String,
        path: String,
    },
    /// A node is scoped (carries a `scope`) but its type is not permitted by the scoping
    /// policy (`scoped_types_whitelist` / `scoped_types_blacklist`, manifest.md). Behaviour is
    /// unchanged — the node is still scoped — but the policy flags it.
    ScopedTypeNotPermitted {
        id: String,
        node_type: String,
        path: String,
    },
}

impl Warning {
    pub fn kind(&self) -> &'static str {
        match self {
            Warning::Orphan { .. } => "orphan",
            Warning::Drift { .. } => "drift",
            Warning::FrontmatterWikilink { .. } => "frontmatter_wikilink",
            Warning::UnknownType { .. } => "unknown_type",
            Warning::ScopedTypeNotPermitted { .. } => "scoped_type_not_permitted",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Warning::Orphan { id, path } => format!("{id}  {path}"),
            Warning::Drift { id, to, path, line } => {
                format!("{id} → {to}  inline-only  {path}:{line}")
            }
            Warning::FrontmatterWikilink { id, field, path } => {
                format!("{id}  field '{field}' uses [[ ]] brackets  {path}")
            }
            Warning::UnknownType {
                id,
                field,
                value,
                path,
            } => {
                format!("{id}  field '{field}': '{value}' — unconfigured type, ignored  {path}")
            }
            Warning::ScopedTypeNotPermitted {
                id,
                node_type,
                path,
            } => {
                format!("{id}  type '{node_type}' is scoped but not permitted by policy  {path}")
            }
        }
    }
}

/// The full result of a check run — the `--json` shape of cli.md §4.2.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckReport {
    pub ok: bool,
    pub violations: Vec<Violation>,
    pub warnings: Vec<Warning>,
}

impl Index {
    /// Run the integrity guards. `config` supplies the type vocabulary (`types`, used to flag
    /// reference-shaped frontmatter values whose type isn't configured and so was ignored) and
    /// the scoping policy (`scoped_types_whitelist`/`blacklist`). Duplicate IDs and dangling
    /// refs are violations; orphans, drift, frontmatter-wikilink, unknown-type, and
    /// scoped-type-not-permitted are warnings (promoted to failures only under `--strict`).
    pub fn check(&self, config: &crate::config::Config) -> Result<CheckReport> {
        let configured: std::collections::HashSet<&str> =
            config.types.iter().map(String::as_str).collect();
        let mut violations = Vec::new();
        let mut warnings = Vec::new();

        // Duplicate IDs: the same composed `type:id` parsed from more than one file.
        let dups: Vec<(String, String)> = self.query_rows(
            "SELECT id, group_concat(path, '\n') FROM node_files
             GROUP BY id HAVING COUNT(*) > 1 ORDER BY id",
            (),
            |r| Ok((col_text(r, 0)?, col_text(r, 1)?)),
        )?;
        for (id, paths) in dups {
            violations.push(Violation::DuplicateId {
                id,
                paths: paths.split('\n').map(str::to_string).collect(),
            });
        }

        // Dangling references: a (resolved) edge whose target is not a node.
        let dangling: Vec<(String, String, String, u32)> = self.query_rows(
            "SELECT from_id, to_id, source_file, line FROM edges
             WHERE to_id NOT IN (SELECT id FROM nodes)
             ORDER BY source_file, line",
            (),
            |r| {
                Ok((
                    col_text(r, 0)?,
                    col_text(r, 1)?,
                    col_text(r, 2)?,
                    col_u32(r, 3)?,
                ))
            },
        )?;
        for (from, to, path, line) in dangling {
            violations.push(Violation::DanglingRef {
                from,
                to,
                path,
                line,
            });
        }

        // Orphans (warning): a node with no inbound or outbound edges.
        let orphans: Vec<(String, String)> = self.query_rows(
            "SELECT id, path FROM nodes
             WHERE id NOT IN (SELECT from_id FROM edges)
               AND id NOT IN (SELECT to_id FROM edges)
             ORDER BY id",
            (),
            |r| Ok((col_text(r, 0)?, col_text(r, 1)?)),
        )?;
        for (id, path) in orphans {
            warnings.push(Warning::Orphan { id, path });
        }

        // Drift (warning): a resolved inline ref whose target is not also a frontmatter
        // edge of the same node. De-duplicated per (from, to); one direction only.
        let drift: Vec<(String, String, String, u32)> = self.query_rows(
            "SELECT e.from_id, e.to_id, MIN(e.source_file), MIN(e.line)
             FROM edges e
             WHERE e.ref_type = 'inline'
               AND NOT EXISTS (
                   SELECT 1 FROM edges f
                   WHERE f.from_id = e.from_id AND f.to_id = e.to_id AND f.ref_type <> 'inline'
               )
             GROUP BY e.from_id, e.to_id
             ORDER BY MIN(e.source_file), MIN(e.line)",
            (),
            |r| {
                Ok((
                    col_text(r, 0)?,
                    col_text(r, 1)?,
                    col_text(r, 2)?,
                    col_u32(r, 3)?,
                ))
            },
        )?;
        for (id, to, path, line) in drift {
            warnings.push(Warning::Drift { id, to, path, line });
        }

        // Frontmatter wikilink trap (warning): a frontmatter value written with `[[ ]]`
        // brackets — detectable from the stored JSON as either a string containing `[[`
        // or a nested array (the unquoted `[[...]]` parses to one). cli.md §6.3.
        let fm_rows: Vec<(String, String, String)> = self.query_rows(
            "SELECT id, path, frontmatter FROM nodes ORDER BY id",
            (),
            |r| Ok((col_text(r, 0)?, col_text(r, 1)?, col_text(r, 2)?)),
        )?;
        for (id, path, fm_json) in fm_rows {
            if let Ok(serde_json::Value::Object(obj)) = serde_json::from_str(&fm_json) {
                for (field, value) in &obj {
                    if looks_like_frontmatter_wikilink(value) {
                        warnings.push(Warning::FrontmatterWikilink {
                            id: id.clone(),
                            field: field.clone(),
                            path: path.clone(),
                        });
                    }
                    // Classification (design.md §6): a candidate reference whose type
                    // isn't declared was *ignored* rather than made an edge — never
                    // silence it (skip the non-reference display/identity fields).
                    if !crate::corpus::frontmatter::NON_EDGE_KEYS.contains(&field.as_str()) {
                        for v in scalar_strings(value) {
                            if let Some(ty) = candidate_type(v)
                                && !configured.contains(ty.as_str())
                            {
                                warnings.push(Warning::UnknownType {
                                    id: id.clone(),
                                    field: field.clone(),
                                    value: v.to_string(),
                                    path: path.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Scoped-type policy (warning): a scoped node (id has a `/`) whose type is not
        // permitted by the whitelist/blacklist. Data-driven scoping is unaffected — this only
        // flags the policy violation.
        let scoped: Vec<(String, String, String)> = self.query_rows(
            "SELECT id, type, path FROM nodes WHERE id LIKE '%/%' ORDER BY id",
            (),
            |r| Ok((col_text(r, 0)?, col_text(r, 1)?, col_text(r, 2)?)),
        )?;
        for (id, node_type, path) in scoped {
            if !config.scoping_permitted(&node_type) {
                warnings.push(Warning::ScopedTypeNotPermitted {
                    id,
                    node_type,
                    path,
                });
            }
        }

        Ok(CheckReport {
            ok: violations.is_empty(),
            violations,
            warnings,
        })
    }
}

/// The string scalars of a frontmatter value (the value itself, or each string element of
/// an array) — what the edge detector scans.
fn scalar_strings(value: &serde_json::Value) -> Vec<&str> {
    match value {
        serde_json::Value::String(s) => vec![s.as_str()],
        serde_json::Value::Array(items) => items.iter().filter_map(|e| e.as_str()).collect(),
        _ => Vec::new(),
    }
}

/// If `value` is a **candidate reference** — it matches the strict target grammar, i.e.
/// identification per design.md §6 — return the node's own type (the last segment's, for
/// a scoped target). This is the *same parser* the edge path uses, so check and build can
/// never disagree on what counts as a reference: a URL, a time, or a colon in prose fails
/// the grammar and is structurally not a candidate (the `url:` fix). Stray `[[ ]]`
/// brackets are stripped first (the frontmatter trap, flagged separately); unresolved
/// `?type:` forms are skipped.
fn candidate_type(value: &str) -> Option<NodeType> {
    let v = value.trim();
    let v = v
        .strip_prefix("[[")
        .and_then(|x| x.strip_suffix("]]"))
        .map(str::trim)
        .unwrap_or(v);
    if v.starts_with('?') {
        return None;
    }
    let id: NodeId = v.parse().ok()?;
    Some(id.node_type)
}

/// Whether a stored frontmatter value bears the `[[ ]]` trap: a string containing the
/// brackets, or a nested array/object (what an unquoted `[[...]]` parses to). Plain
/// arrays of scalars (normal edge lists like `[person:a, dept:b]`) are fine.
fn looks_like_frontmatter_wikilink(value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match value {
        Value::String(s) => s.contains("[[") || s.contains("]]"),
        Value::Array(items) => items.iter().any(|e| {
            matches!(e, Value::Array(_) | Value::Object(_)) || looks_like_frontmatter_wikilink(e)
        }),
        _ => false,
    }
}
