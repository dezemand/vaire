//! `vaire index [--full]` (cli.md §4.1). Maintain command — not on the MCP surface.

use crate::commands::Ctx;
use crate::error::Result;
use crate::index::build::{self, IndexSummary, Mode};

pub fn run(ctx: &Ctx, full: bool, working_tree: bool, re_embed: bool) -> Result<IndexSummary> {
    let embedder = ctx.embedder()?;
    if re_embed {
        return build::reembed(&ctx.repo, embedder.as_ref());
    }
    let mode = if working_tree {
        Mode::WorkingTree
    } else if full {
        Mode::Full
    } else {
        Mode::Incremental
    };
    build::run(&ctx.repo, &ctx.config, embedder.as_ref(), mode)
}
