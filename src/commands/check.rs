//! `vaire check [--strict]` (cli.md §4.2). Maintain command — not on the MCP surface.
//!
//! Read-only. The caller (binary) maps a non-clean report to exit `6`. `--strict`
//! promotes orphan warnings to violations.

use crate::commands::Ctx;
use crate::error::Result;
use crate::index::build::{self, Mode};
use crate::index::check::CheckReport;

pub fn run(ctx: &Ctx, strict: bool, working_tree: bool) -> Result<(CheckReport, bool)> {
    // `--working-tree` reindexes from the working tree first, so the checks see
    // uncommitted edits (the index then reflects the working tree, not the last commit).
    if working_tree {
        let embedder = ctx.embedder()?;
        build::run(&ctx.repo, &ctx.config, embedder.as_ref(), Mode::WorkingTree)?;
    }
    let index = ctx.open_index()?;
    let report = index.check(&ctx.config.id_prefixes)?;
    let failed = !report.violations.is_empty() || (strict && !report.warnings.is_empty());
    Ok((report, failed))
}
