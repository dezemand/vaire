//! `vaire check [--strict] [--working-tree] [--no-deps]` (cli.md §4.2). Maintain
//! command — not on the MCP surface.
//!
//! The caller (binary) maps a non-clean report to exit `6`. `--strict` promotes
//! warnings to violations.
//!
//! Since M5 the run starts with the linked-dependency **ensure pass** (same as `vaire
//! index`; near-no-op when fresh, `--no-deps` skips) so the resolution lints judge
//! commit-fresh dependency indexes — a cold clone can run `vaire check` first. The
//! resolution-dependent lints compose here, on top of the per-index [`Index::check`]:
//! dangling cross-package references (error), missing dependencies (error, once per
//! name), unused dependencies (warning), and version mismatches (warning — surfacing
//! only, enforcement is v0.3).

use std::rc::Rc;

use crate::commands::Ctx;
use crate::error::{Result, VaireError};
use crate::index::build::{self, Mode};
use crate::index::check::{CheckReport, Violation, Warning};
use crate::model::id::NodeId;
use crate::workspace::resolver;

pub fn run(
    ctx: &Ctx,
    strict: bool,
    working_tree: bool,
    no_deps: bool,
) -> Result<(CheckReport, bool)> {
    // `--working-tree` reindexes from the working tree first, so the checks see
    // uncommitted edits (the index then reflects the working tree, not the last commit).
    if working_tree {
        let embedder = ctx.embedder()?;
        build::run(&ctx.repo, &ctx.config, embedder, Mode::WorkingTree)?;
    }
    // Ensure linked dependencies are indexed before judging references into them
    // (check is a maintain command — building derived caches is in its charter, like
    // `--working-tree` above).
    if !no_deps && !ctx.config.dependencies.is_empty() {
        let embedder = ctx.embedder()?;
        crate::commands::index::ensure_deps(ctx, embedder)?;
    }

    let index = ctx.open_index()?;
    let mut report = index.check(&ctx.config)?;
    if !ctx.config.dependencies.is_empty() {
        resolution_lints(ctx, &index, &mut report)?;
        report.ok = report.violations.is_empty();
    }
    let failed = !report.violations.is_empty() || (strict && !report.warnings.is_empty());
    Ok((report, failed))
}

/// The lints that need actual cross-package resolution (packages.md §8), composed over
/// the per-index checks. Iterates only this package's edge table plus point lookups in
/// dependency indexes — no graph traversal, so the acme-core↔acme-web dependency cycle
/// terminates structurally.
fn resolution_lints(
    ctx: &Ctx,
    index: &crate::index::Index,
    report: &mut CheckReport,
) -> Result<()> {
    let ws = ctx.workspace()?;
    let current = ws.current();

    // Which declared dependencies are unavailable — reported once each, their edges
    // skipped by the dangling pass (undeclared aliases stay `undeclared_import`, M4).
    let mut missing: std::collections::BTreeMap<String, String> = Default::default();
    // Unavailable packages hit mid-tombstone-chain: note → via-context (deduped by note).
    let mut chain_missing: std::collections::BTreeMap<String, String> = Default::default();
    for name in ctx.config.dependencies.keys() {
        if let Err(e) = ws.locate(&current, name) {
            missing.insert(name.clone(), e.to_string());
        }
    }

    // Dangling cross-package references: a declared, available alias whose target —
    // after tombstone-following in the OWNING package's context — does not exist.
    for edge in index.cross_edges()? {
        let (alias, to_id) = (edge.to_package, edge.to_id);
        if !ctx.config.dependencies.contains_key(&alias) || missing.contains_key(&alias) {
            continue;
        }
        let target: NodeId = format!("@{alias}/{to_id}")
            .parse()
            .unwrap_or_else(|_| NodeId::parse_stored(&to_id).with_package(alias.clone()));
        match resolver::resolve(ws, Rc::clone(&current), &target) {
            Ok(_) => {}
            Err(VaireError::IdNotFound(_)) => report.violations.push(Violation::DanglingRef {
                from: edge.from_id,
                to: format!("@{alias}/{to_id}"),
                path: edge.source_file,
                line: edge.line,
            }),
            // A tombstone chain reached an unavailable package mid-flight: fold into
            // missing_dependency — keyed by the MESSAGE (which names the unavailable
            // package), so N edges through the same broken chain report once; the
            // via-context lives in the value.
            Err(VaireError::Dependency(note)) => {
                chain_missing
                    .entry(note)
                    .or_insert_with(|| format!("(via @{alias}/{to_id})"));
            }
            Err(e) => return Err(e),
        }
    }

    for (package, note) in missing {
        report
            .violations
            .push(Violation::MissingDependency { package, note });
    }
    for (note, via) in chain_missing {
        report
            .violations
            .push(Violation::MissingDependency { package: via, note });
    }

    // Unused + version-mismatch, per declared dependency.
    for (name, constraint) in &ctx.config.dependencies {
        if !index.references_package(name)? {
            report.warnings.push(Warning::UnusedDependency {
                package: name.clone(),
            });
        }
        if let Ok(handle) = ws.locate(&current, name)
            && let Some(major) = constraint.strip_prefix('^')
            && handle.config.version.split('.').next() != Some(major)
        {
            report.warnings.push(Warning::DependencyVersionMismatch {
                package: name.clone(),
                constraint: constraint.clone(),
                version: handle.config.version.clone(),
            });
        }
    }

    Ok(())
}
