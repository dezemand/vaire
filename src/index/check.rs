//! Integrity guards — `vaire check` (cli.md §4.2, design.md §9).
//!
//! ID-based discovery makes two checks fall out for free (unique IDs, resolvable refs)
//! plus two structural ones (drift, orphans). Read-only; exits `6` on any violation
//! (or any warning under `--strict`).

use crate::error::Result;
use crate::index::db::{Index, col_opt_text, col_text, col_u32};
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
    /// An `@pkg/…` reference whose package is not in the manifest `[dependencies]`
    /// (manifest.md §5: undeclared import). A pure table check — no cross-package
    /// *resolution* is needed to know the dependency was never declared.
    UndeclaredImport {
        package: String,
        from: String,
        to: String,
        path: String,
        line: u32,
    },
    /// A declared dependency that is unavailable — not linked, a broken link, or a name
    /// mismatch — so its references cannot be verified at all. Reported once per
    /// dependency; its edges are skipped by the dangling pass (no spam). The note
    /// carries the exact fix (cli.md §6.5).
    MissingDependency { package: String, note: String },
}

impl Violation {
    /// The stable `kind` string (matches the JSON tag).
    pub fn kind(&self) -> &'static str {
        match self {
            Violation::DuplicateId { .. } => "duplicate_id",
            Violation::DanglingRef { .. } => "dangling_ref",
            Violation::UndeclaredImport { .. } => "undeclared_import",
            Violation::MissingDependency { .. } => "missing_dependency",
        }
    }

    /// Whether this violation implicates `path` (package-root-relative, forward slashes).
    ///
    /// The filter behind `release`'s post-write check: the release tree is clean by gate,
    /// so a violation naming the record just written is the summary's doing, and saying so
    /// beats re-printing a corpus-wide report the maintainer already passed a moment ago.
    /// A `MissingDependency` names no file — it is about the manifest — so it belongs to
    /// nobody's path.
    pub fn involves(&self, path: &str) -> bool {
        match self {
            Violation::DuplicateId { paths, .. } => paths.iter().any(|p| p == path),
            Violation::DanglingRef { path: p, .. }
            | Violation::UndeclaredImport { path: p, .. } => p == path,
            Violation::MissingDependency { .. } => false,
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
            Violation::UndeclaredImport {
                package,
                from,
                to,
                path,
                line,
            } => {
                format!("{from} → {to}  package '{package}' not in [dependencies]  {path}:{line}")
            }
            Violation::MissingDependency { package, note } => {
                format!("'{package}' unavailable — {note}")
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
    /// A node whose *declared* id (or scope) falls outside the strict reference target
    /// grammar (design.md §6) — e.g. `id: Jane_Doe`. The file still indexes (files are
    /// truth), but no reference can ever address it: identification is by shape, so a
    /// target naming this node fails to parse. Surfaced instead of left as a silent trap.
    UnreferenceableId {
        id: String,
        path: String,
        reason: String,
    },
    /// A `vaire/` marker in a diagram source whose target does not parse as a reference
    /// (design.md §6) — `vaire/team alpha`, `vaire/Team:Alpha`, a stray trailing bracket.
    ///
    /// Warned rather than failed, for the same reason [`Warning::UnknownType`] is: the
    /// marker was *ignored*, so the corpus is intact and only the author's intent was
    /// lost. It has to be said out loud all the same — a diagram has no loose-end form, so
    /// nothing else in the system would ever mention it again.
    MalformedDiagramRef {
        id: String,
        raw: String,
        path: String,
        line: u32,
    },
    /// A declared dependency no reference ever uses (manifest.md §5: unused). Pure
    /// manifest + edge-table check.
    UnusedDependency { package: String },
    /// A linked dependency whose declared MAJOR falls outside this package's `^N`
    /// constraint. Surfaced only — version *enforcement* is explicitly out of scope for
    /// v0.2 (issue #2 cut line).
    DependencyVersionMismatch {
        package: String,
        constraint: String,
        version: String,
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
            Warning::UnreferenceableId { .. } => "unreferenceable_id",
            Warning::MalformedDiagramRef { .. } => "malformed_diagram_ref",
            Warning::UnusedDependency { .. } => "unused_dependency",
            Warning::DependencyVersionMismatch { .. } => "dependency_version_mismatch",
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
            Warning::UnreferenceableId { id, path, reason } => {
                format!("{id}  no reference can address this id ({reason})  {path}")
            }
            Warning::MalformedDiagramRef {
                id,
                raw,
                path,
                line,
            } => {
                format!("{id}  'vaire/{raw}' is not a reference target, ignored  {path}:{line}")
            }
            Warning::UnusedDependency { package } => {
                format!("'{package}' is declared but never referenced")
            }
            Warning::DependencyVersionMismatch {
                package,
                constraint,
                version,
            } => {
                format!(
                    "'{package}' declares version {version}, outside this package's {constraint}"
                )
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
    /// candidate references whose type isn't configured and so was ignored), the declared
    /// `dependencies` (used to flag an `@pkg/` import of an undeclared package), and the
    /// scoping policy (`scoped_types_whitelist`/`blacklist`). Duplicate IDs, dangling refs,
    /// and undeclared imports are violations; orphans, drift, frontmatter-wikilink,
    /// unknown-type, scoped-type-not-permitted, and unreferenceable-id are warnings (promoted
    /// to failures only under `--strict`).
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

        // Dangling references: a (resolved) *local* edge whose target is not a node. A
        // cross-package target (to_package set) can't be checked until workspace
        // resolution (M5), so it is excluded here — undeclared_import guards it instead.
        //
        // **A release record's computed edges are exempt** — those and nothing else. A
        // record says what a release published, which is a statement about the past; an
        // entity deleted afterwards makes that statement *historical*, not wrong. Nothing
        // here could be actionable either way: records are immutable, so the edge cannot
        // be corrected, and it had no author to have mistyped it — the classifier wrote it
        // from the index. Without this, one hard deletion would fail `check` forever, and
        // since `release` gates on `check`, the package could never be released again.
        // (The supported way to retire an entity is a `superseded_by:` tombstone, which
        // keeps the node addressable and never reaches this rule.)
        //
        // The exemption is keyed on the edge also being carried by a computed frontmatter
        // key, which is what keeps it honest: it covers the classifier's own entries and
        // their twins in the rendered `## Added` sections, while an address a `--summary`
        // *invented* — inline, with no computed counterpart — still dangles and still
        // refuses the release. That distinction is the whole reason outside prose is safe
        // to admit into a record at all.
        let dangling: Vec<(String, String, String, u32)> = self.query_rows(
            "SELECT from_id, to_id, source_file, line FROM edges e
             WHERE to_package IS NULL AND to_id NOT IN (SELECT id FROM nodes)
               AND NOT (
                 from_id IN (SELECT id FROM nodes WHERE type = ?1)
                 AND EXISTS (
                   SELECT 1 FROM edges c
                   WHERE c.from_id = e.from_id AND c.to_id = e.to_id
                     AND c.to_package IS NULL
                     AND c.ref_type IN ('added', 'changed', 'retired')
                 )
               )
             ORDER BY source_file, line",
            [config.release_type.as_str()],
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

        // Undeclared import (violation): an `@pkg/…` edge whose package is not declared in
        // the manifest `[dependencies]` (manifest.md §5). Pure table check — the package is
        // recorded on the edge (M4), so this needs no cross-package resolution.
        let imports: Vec<(String, String, String, String, u32)> = self.query_rows(
            "SELECT to_package, from_id, to_id, source_file, line FROM edges
             WHERE to_package IS NOT NULL ORDER BY source_file, line",
            (),
            |r| {
                Ok((
                    col_text(r, 0)?,
                    col_text(r, 1)?,
                    col_text(r, 2)?,
                    col_text(r, 3)?,
                    col_u32(r, 4)?,
                ))
            },
        )?;
        for (package, from, to_id, path, line) in imports {
            if !config.dependencies.contains_key(&package) {
                violations.push(Violation::UndeclaredImport {
                    to: display_target(&to_id, Some(&package)),
                    package,
                    from,
                    path,
                    line,
                });
            }
        }

        // Markers in diagram sources that did not parse as reference targets (warning).
        // Recorded at index time because nothing else would remember them: a diagram has
        // no loose-end form, so this is where "a typo must not evaporate" is honoured.
        let malformed: Vec<(String, String, String, u32)> = self.query_rows(
            "SELECT from_id, raw, source_file, line FROM malformed_diagram_refs
             ORDER BY source_file, line, raw",
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
        for (id, raw, path, line) in malformed {
            warnings.push(Warning::MalformedDiagramRef {
                id,
                raw,
                path,
                line,
            });
        }

        // Orphans (warning): a node with no inbound or outbound edges. Inbound counts only
        // *local* edges — a cross-package edge's bare to_id could coincide with a local id
        // but points at another package's node, not this one.
        let orphans: Vec<(String, String)> = self.query_rows(
            "SELECT id, path FROM nodes
             WHERE id NOT IN (SELECT from_id FROM edges)
               AND id NOT IN (SELECT to_id FROM edges WHERE to_package IS NULL)
             ORDER BY id",
            (),
            |r| Ok((col_text(r, 0)?, col_text(r, 1)?)),
        )?;
        for (id, path) in orphans {
            warnings.push(Warning::Orphan { id, path });
        }

        // Drift (warning): a resolved inline ref whose target is not also a frontmatter
        // edge of the same node. De-duplicated per (from, to); one direction only.
        let drift: Vec<(String, String, Option<String>, String, u32)> = self.query_rows(
            "SELECT e.from_id, e.to_id, e.to_package, MIN(e.source_file), MIN(e.line)
             FROM edges e
             WHERE e.ref_type = 'inline'
               AND NOT EXISTS (
                   SELECT 1 FROM edges f
                   WHERE f.from_id = e.from_id AND f.to_id = e.to_id
                     AND f.to_package IS e.to_package AND f.ref_type NOT IN ('inline', 'diagram')
               )
             GROUP BY e.from_id, e.to_id, e.to_package
             ORDER BY MIN(e.source_file), MIN(e.line)",
            (),
            |r| {
                Ok((
                    col_text(r, 0)?,
                    col_text(r, 1)?,
                    col_opt_text(r, 2)?,
                    col_text(r, 3)?,
                    col_u32(r, 4)?,
                ))
            },
        )?;
        for (id, to, to_package, path, line) in drift {
            let to = display_target(&to, to_package.as_deref());
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

        // Unreferenceable id (warning): the node indexed — files are truth — but its
        // declared id (or scope) falls outside the strict target grammar (design.md §6),
        // so no reference can ever parse to it. Surface the trap instead of leaving it
        // silent.
        let all_ids: Vec<(String, String)> =
            self.query_rows("SELECT id, path FROM nodes ORDER BY id", (), |r| {
                Ok((col_text(r, 0)?, col_text(r, 1)?))
            })?;
        for (id, path) in all_ids {
            if let Err(e) = id.parse::<NodeId>() {
                warnings.push(Warning::UnreferenceableId {
                    id,
                    path,
                    reason: e.to_string(),
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

/// Render a stored edge target for display: a local target is its bare `to_id`, a
/// cross-package target regains its `@pkg/` qualifier (`to_id` is the within-package
/// address; the package lives in a separate column).
fn display_target(to_id: &str, to_package: Option<&str>) -> String {
    match to_package {
        Some(pkg) => format!("@{pkg}/{to_id}"),
        None => to_id.to_string(),
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
    // A cross-package candidate is classified by its OWNING package's vocabulary, never
    // this one's (same rule as the edge gate in build.rs) — the resolution lints judge
    // it instead.
    if id.package().is_some() {
        return None;
    }
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
